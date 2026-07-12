use crate::auth::{jwt_tier, primary_entry};
use crate::error::{AppError, AppResult};
use crate::store::{get_access_token, load_account_snapshot, update_quota, update_subscription};
use crate::types::QuotaInfo;
use chrono::{DateTime, Utc};
use serde::Deserialize;

const BILLING_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing";
const USER_URL: &str = "https://cli-chat-proxy.grok.com/v1/user";
const USER_SUB_URL: &str = "https://cli-chat-proxy.grok.com/v1/user?include=subscription";

#[derive(Debug, Deserialize)]
struct BillingResponse {
    config: BillingConfig,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BillingConfig {
    monthly_limit: MoneyVal,
    used: MoneyVal,
    #[serde(default)]
    on_demand_cap: Option<MoneyVal>,
    /// Optional: "WEEKLY" | "MONTHLY" (present on some API versions / plan types)
    #[serde(default)]
    current_period: Option<String>,
    billing_period_start: String,
    #[serde(default)]
    billing_period_end: Option<String>,
    /// Optional weekly fields if API ever returns them
    #[serde(default)]
    weekly_limit: Option<MoneyVal>,
    #[serde(default)]
    weekly_used: Option<MoneyVal>,
}

#[derive(Debug, Deserialize)]
struct MoneyVal {
    val: f64,
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
    /// e.g. "GrokPro" when called with ?include=subscription
    #[serde(default)]
    pub subscription_tiers: Option<String>,
    /// Not currently returned by API, reserved if xAI adds it later
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

/// Infer weekly vs monthly from API enum or period length.
fn period_kind(current_period: Option<&str>, start: &str, end: &str) -> (String, String) {
    if let Some(cp) = current_period {
        let u = cp.to_uppercase();
        if u.contains("WEEK") {
            return ("weekly".into(), "Weekly".into());
        }
        if u.contains("MONTH") {
            return ("monthly".into(), "Monthly".into());
        }
    }
    if let (Some(s), Some(e)) = (parse_rfc3339(start), parse_rfc3339(end)) {
        let days = (e - s).num_days().abs();
        // ~7 days → weekly, otherwise treat as monthly
        if days > 0 && days <= 10 {
            return ("weekly".into(), "Weekly".into());
        }
    }
    ("monthly".into(), "Monthly".into())
}

fn days_until_reset(end: &str) -> i64 {
    let Some(e) = parse_rfc3339(end) else {
        return 0;
    };
    let now = Utc::now();
    (e - now).num_days().max(0)
}

pub fn fetch_quota_for_token(token: &str) -> AppResult<QuotaInfo> {
    let client = client()?;
    let resp = client
        .get(BILLING_URL)
        .headers(auth_headers(token))
        .send()?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(AppError::msg(format!(
            "Billing API {status}: {}",
            body.chars().take(200).collect::<String>()
        )));
    }

    let data: BillingResponse = resp.json()?;
    let cfg = data.config;

    // Prefer explicit weekly fields when present; else use monthlyLimit/used
    // (API reuses monthlyLimit even for weekly periods on some tiers).
    let (used, limit) = if let (Some(wu), Some(wl)) = (&cfg.weekly_used, &cfg.weekly_limit) {
        (wu.val, wl.val)
    } else {
        (cfg.used.val, cfg.monthly_limit.val)
    };

    let end = cfg
        .billing_period_end
        .clone()
        .unwrap_or_else(|| cfg.billing_period_start.clone());
    let (kind, label) = period_kind(
        cfg.current_period.as_deref(),
        &cfg.billing_period_start,
        &end,
    );

    let on_demand_cap = cfg.on_demand_cap.map(|v| v.val).unwrap_or(0.0);
    let percent_used = if limit > 0.0 {
        (used / limit) * 100.0
    } else {
        0.0
    };

    Ok(QuotaInfo {
        used,
        monthly_limit: limit,
        on_demand_cap,
        billing_period_start: cfg.billing_period_start,
        billing_period_end: end.clone(),
        percent_used,
        fetched_at: chrono::Utc::now().to_rfc3339(),
        period_kind: kind,
        period_label: label,
        days_until_reset: days_until_reset(&end),
        resets_at: end,
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

/// Fetch plan name via ?include=subscription → subscriptionTier (e.g. GrokPro).
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

    // Plan name (subscriptionTier) + optional expiry
    let (sub_tier, plan_exp) = match fetch_subscription_for_token(&token) {
        Ok(u) => (u.subscription_tiers.clone(), plan_expires_from_user(&u)),
        Err(_) => (None, None),
    };

    update_quota(user_id, quota.clone(), tier)?;
    let _ = update_subscription(user_id, sub_tier, plan_exp);
    Ok(quota)
}
