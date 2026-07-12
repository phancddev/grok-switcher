use crate::auth::{jwt_tier, primary_entry};
use crate::error::{AppError, AppResult};
use crate::store::{get_access_token, load_account_snapshot, update_quota, update_subscription};
use crate::types::{PeriodQuota, QuotaInfo};
use chrono::{DateTime, Utc};
use serde::Deserialize;

const BILLING_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing";
const BILLING_CREDITS_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing?format=credits";
const USER_URL: &str = "https://cli-chat-proxy.grok.com/v1/user";
const USER_SUB_URL: &str = "https://cli-chat-proxy.grok.com/v1/user?include=subscription";

// ── default format (monthly dollars/credits style) ──────────────────────────

#[derive(Debug, Deserialize)]
struct BillingResponse {
    config: BillingConfig,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BillingConfig {
    #[serde(default)]
    monthly_limit: Option<MoneyVal>,
    #[serde(default)]
    used: Option<MoneyVal>,
    #[serde(default)]
    on_demand_cap: Option<MoneyVal>,
    #[serde(default)]
    #[allow(dead_code)]
    current_period: Option<serde_json::Value>,
    #[serde(default)]
    billing_period_start: Option<String>,
    #[serde(default)]
    billing_period_end: Option<String>,
    #[serde(default)]
    weekly_limit: Option<MoneyVal>,
    #[serde(default)]
    weekly_used: Option<MoneyVal>,
    #[serde(default)]
    credit_usage_percent: Option<f64>,
    #[serde(default)]
    #[allow(dead_code)]
    on_demand_used: Option<MoneyVal>,
    #[serde(default)]
    product_usage: Option<Vec<ProductUsage>>,
    #[serde(default)]
    #[allow(dead_code)]
    prepaid_balance: Option<MoneyVal>,
    #[serde(default)]
    #[allow(dead_code)]
    is_unified_billing_user: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct MoneyVal {
    val: f64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProductUsage {
    product: Option<String>,
    #[serde(default)]
    usage_percent: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreditsPeriod {
    #[serde(default)]
    r#type: Option<String>,
    #[serde(default)]
    start: Option<String>,
    #[serde(default)]
    end: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInfo {
    #[allow(dead_code)]
    pub user_id: Option<String>,
    pub email: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    #[allow(dead_code)]
    pub has_grok_code_access: Option<bool>,
    #[serde(default)]
    pub subscription_tiers: Option<String>,
    #[serde(default)]
    pub subscription_expires_at: Option<String>,
    #[serde(default)]
    pub plan_expires_at: Option<String>,
    #[serde(default)]
    pub expires_at: Option<String>,
}

fn client() -> AppResult<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .user_agent("grok-switcher/0.1")
        .build()?)
}

fn auth_headers(token: &str) -> reqwest::header::HeaderMap {
    use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
    let mut h = HeaderMap::new();
    h.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {token}")).unwrap_or(HeaderValue::from_static("")),
    );
    h.insert(
        "X-XAI-Token-Auth",
        HeaderValue::from_static("xai-grok-cli"),
    );
    h.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    h
}

fn parse_rfc3339(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

fn days_until(end: &str) -> i64 {
    parse_rfc3339(end)
        .map(|e| (e - Utc::now()).num_days().max(0))
        .unwrap_or(0)
}

fn percent(used: f64, limit: f64) -> f64 {
    if limit > 0.0 {
        (used / limit) * 100.0
    } else {
        0.0
    }
}

fn period_quota(
    kind: &str,
    label: &str,
    used: f64,
    limit: f64,
    start: &str,
    end: &str,
    source: &str,
) -> PeriodQuota {
    PeriodQuota {
        kind: kind.into(),
        label: label.into(),
        used,
        limit,
        percent_used: percent(used, limit),
        period_start: start.into(),
        period_end: end.into(),
        resets_at: end.into(),
        days_until_reset: days_until(end),
        source: source.into(),
    }
}

fn get_json(url: &str, token: &str) -> AppResult<serde_json::Value> {
    let client = client()?;
    let resp = client.get(url).headers(auth_headers(token)).send()?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(AppError::msg(format!(
            "Billing API {status}: {}",
            body.chars().take(200).collect::<String>()
        )));
    }
    Ok(resp.json()?)
}

fn parse_credits_period(v: &serde_json::Value) -> Option<CreditsPeriod> {
    let cp = v.get("config")?.get("currentPeriod")?;
    serde_json::from_value(cp.clone()).ok()
}

/// GrokBuild product usage percent from format=credits response.
fn grok_build_percent(cfg: &BillingConfig) -> Option<f64> {
    if let Some(list) = &cfg.product_usage {
        for p in list {
            let name = p.product.as_deref().unwrap_or("");
            if name.eq_ignore_ascii_case("GrokBuild") || name.eq_ignore_ascii_case("Grok Build") {
                if let Some(pct) = p.usage_percent {
                    return Some(pct);
                }
            }
        }
    }
    cfg.credit_usage_percent
}

/// Fetch default monthly billing + format=credits weekly usage and merge.
pub fn fetch_quota_for_token(token: &str) -> AppResult<QuotaInfo> {
    // 1) Monthly-style payload (used / monthlyLimit)
    let monthly_raw = get_json(BILLING_URL, token)?;
    let monthly: BillingResponse = serde_json::from_value(monthly_raw.clone()).map_err(|e| {
        AppError::msg(format!("Failed to parse monthly billing: {e}"))
    })?;
    let mcfg = monthly.config;

    let used = mcfg.used.as_ref().map(|m| m.val).unwrap_or(0.0);
    let monthly_limit = mcfg.monthly_limit.as_ref().map(|m| m.val).unwrap_or(0.0);
    let on_demand_cap = mcfg.on_demand_cap.as_ref().map(|m| m.val).unwrap_or(0.0);
    let m_start = mcfg
        .billing_period_start
        .clone()
        .unwrap_or_default();
    let m_end = mcfg
        .billing_period_end
        .clone()
        .unwrap_or_else(|| m_start.clone());

    let monthly_period = if monthly_limit > 0.0 || used > 0.0 {
        Some(period_quota(
            "monthly",
            "Monthly",
            used,
            monthly_limit,
            &m_start,
            &m_end,
            "api",
        ))
    } else {
        None
    };

    // 2) Credits format → real weekly window + usage percent
    let mut weekly_period: Option<PeriodQuota> = None;
    if let Ok(credits_raw) = get_json(BILLING_CREDITS_URL, token) {
        if let Ok(credits) = serde_json::from_value::<BillingResponse>(credits_raw.clone()) {
            let ccfg = credits.config;
            let period = parse_credits_period(&credits_raw);
            let (w_start, w_end, is_weekly) = if let Some(ref p) = period {
                let start = p.start.clone().unwrap_or_default();
                let end = p.end.clone().unwrap_or_else(|| start.clone());
                let weekly = p
                    .r#type
                    .as_deref()
                    .map(|t| t.to_uppercase().contains("WEEK"))
                    .unwrap_or(true);
                (start, end, weekly)
            } else {
                let start = ccfg.billing_period_start.clone().unwrap_or_default();
                let end = ccfg
                    .billing_period_end
                    .clone()
                    .unwrap_or_else(|| start.clone());
                (start, end, true)
            };

            // Absolute weekly used/limit if present; else percent-based (used/limit as 0–100)
            if let (Some(wu), Some(wl)) = (&ccfg.weekly_used, &ccfg.weekly_limit) {
                weekly_period = Some(period_quota(
                    "weekly",
                    "Weekly",
                    wu.val,
                    wl.val,
                    &w_start,
                    &w_end,
                    "api",
                ));
            } else if let Some(pct) = grok_build_percent(&ccfg) {
                // format=credits returns percent only for weekly (e.g. 73.0)
                // Represent as used=pct, limit=100 so progress bar works with real API %.
                weekly_period = Some(PeriodQuota {
                    kind: "weekly".into(),
                    label: "Weekly".into(),
                    used: pct,
                    limit: 100.0,
                    percent_used: pct,
                    period_start: w_start.clone(),
                    period_end: w_end.clone(),
                    resets_at: w_end.clone(),
                    days_until_reset: days_until(&w_end),
                    source: "api".into(),
                });
                let _ = is_weekly;
            }
        }
    }

    // Fallback: if credits format failed but default had weekly absolute fields
    if weekly_period.is_none() {
        if let (Some(wu), Some(wl)) = (&mcfg.weekly_used, &mcfg.weekly_limit) {
            weekly_period = Some(period_quota(
                "weekly",
                "Weekly",
                wu.val,
                wl.val,
                &m_start,
                &m_end,
                "api",
            ));
        }
    }

    let (kind, label, resets, days) = if let Some(ref w) = weekly_period {
        (
            "weekly".to_string(),
            "Weekly".to_string(),
            w.resets_at.clone(),
            w.days_until_reset,
        )
    } else {
        (
            "monthly".to_string(),
            "Monthly".to_string(),
            m_end.clone(),
            days_until(&m_end),
        )
    };

    Ok(QuotaInfo {
        used,
        monthly_limit,
        on_demand_cap,
        billing_period_start: m_start,
        billing_period_end: m_end.clone(),
        percent_used: percent(used, monthly_limit),
        fetched_at: Utc::now().to_rfc3339(),
        period_kind: kind,
        period_label: label,
        days_until_reset: days,
        resets_at: resets,
        monthly: monthly_period,
        weekly: weekly_period,
    })
}

pub fn fetch_user_for_token(token: &str) -> AppResult<UserInfo> {
    let client = client()?;
    let resp = client
        .get(USER_URL)
        .headers(auth_headers(token))
        .send()?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(AppError::msg(format!(
            "User API {status}: {}",
            body.chars().take(200).collect::<String>()
        )));
    }
    Ok(resp.json()?)
}

pub fn fetch_subscription_for_token(token: &str) -> AppResult<UserInfo> {
    let client = client()?;
    let resp = client
        .get(USER_SUB_URL)
        .headers(auth_headers(token))
        .send()?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(AppError::msg(format!(
            "User subscription API {status}: {}",
            body.chars().take(200).collect::<String>()
        )));
    }
    Ok(resp.json()?)
}

pub fn plan_expires_from_user(user: &UserInfo) -> Option<String> {
    user.subscription_expires_at
        .clone()
        .or_else(|| user.plan_expires_at.clone())
        .or_else(|| user.expires_at.clone())
        .filter(|s| !s.is_empty())
}

pub fn refresh_quota(user_id: &str) -> AppResult<QuotaInfo> {
    let token = get_access_token(user_id)?;
    let quota = fetch_quota_for_token(&token)?;

    let auth = load_account_snapshot(user_id)?;
    let (_, entry) = primary_entry(&auth)?;
    let tier = jwt_tier(&entry.key);

    let (sub_tier, plan_exp) = match fetch_subscription_for_token(&token) {
        Ok(u) => (u.subscription_tiers.clone(), plan_expires_from_user(&u)),
        Err(_) => (None, None),
    };

    update_quota(user_id, quota.clone(), tier)?;
    let _ = update_subscription(user_id, sub_tier, plan_exp);
    Ok(quota)
}
