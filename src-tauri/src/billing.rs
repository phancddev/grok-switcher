use crate::auth::{jwt_tier, primary_entry};
use crate::error::{AppError, AppResult};
use crate::store::{
    get_access_token, load_account_snapshot, load_meta, save_meta, update_quota, update_subscription,
};
use crate::types::{PeriodQuota, QuotaInfo, WeekTracker};
use chrono::{DateTime, Datelike, Duration, Utc};
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
    #[serde(default)]
    current_period: Option<String>,
    billing_period_start: String,
    #[serde(default)]
    billing_period_end: Option<String>,
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

fn days_until(end: &DateTime<Utc>) -> i64 {
    (*end - Utc::now()).num_days().max(0)
}

fn percent(used: f64, limit: f64) -> f64 {
    if limit > 0.0 {
        (used / limit) * 100.0
    } else {
        0.0
    }
}

/// ISO week Monday 00:00 UTC → next Monday 00:00 UTC, key "YYYY-Www"
fn iso_week_bounds(now: DateTime<Utc>) -> (String, DateTime<Utc>, DateTime<Utc>) {
    let date = now.date_naive();
    let weekday = date.weekday().num_days_from_monday() as i64; // Mon=0
    let week_start_date = date - Duration::days(weekday);
    let week_start = week_start_date
        .and_hms_opt(0, 0, 0)
        .map(|ndt| DateTime::<Utc>::from_naive_utc_and_offset(ndt, Utc))
        .unwrap_or(now);
    let week_end = week_start + Duration::days(7);
    let iso = date.iso_week();
    let key = format!("{:04}-W{:02}", iso.year(), iso.week());
    (key, week_start, week_end)
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
    let end_dt = parse_rfc3339(end);
    PeriodQuota {
        kind: kind.into(),
        label: label.into(),
        used,
        limit,
        percent_used: percent(used, limit),
        period_start: start.into(),
        period_end: end.into(),
        resets_at: end.into(),
        days_until_reset: end_dt.map(|e| days_until(&e)).unwrap_or(0),
        source: source.into(),
    }
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
    let used = cfg.used.val;
    let monthly_limit = cfg.monthly_limit.val;
    let on_demand_cap = cfg.on_demand_cap.map(|v| v.val).unwrap_or(0.0);
    let end = cfg
        .billing_period_end
        .clone()
        .unwrap_or_else(|| cfg.billing_period_start.clone());
    let start = cfg.billing_period_start.clone();

    // API may return currentPeriod WEEKLY|MONTHLY; GrokPro usually omits it (monthly).
    let api_is_weekly = cfg
        .current_period
        .as_deref()
        .map(|s| s.to_uppercase().contains("WEEK"))
        .unwrap_or(false);

    let monthly = if api_is_weekly {
        // Entire response is weekly — still surface a monthly-style bar for consistency
        // using the same numbers labeled monthly only if period is long; else skip duplicate.
        let days = parse_rfc3339(&start)
            .and_then(|s| parse_rfc3339(&end).map(|e| (e - s).num_days().abs()))
            .unwrap_or(30);
        if days > 14 {
            Some(period_quota(
                "monthly",
                "Monthly",
                used,
                monthly_limit,
                &start,
                &end,
                "api",
            ))
        } else {
            None
        }
    } else {
        Some(period_quota(
            "monthly",
            "Monthly",
            used,
            monthly_limit,
            &start,
            &end,
            "api",
        ))
    };

    // Weekly from API fields if present
    let weekly_from_api = if let (Some(wu), Some(wl)) = (&cfg.weekly_used, &cfg.weekly_limit) {
        // Derive week bounds from calendar if period not weekly
        let (key, ws, we) = iso_week_bounds(Utc::now());
        let _ = key;
        Some(period_quota(
            "weekly",
            "Weekly",
            wu.val,
            wl.val,
            &ws.to_rfc3339(),
            &we.to_rfc3339(),
            "api",
        ))
    } else if api_is_weekly {
        Some(period_quota(
            "weekly",
            "Weekly",
            used,
            monthly_limit,
            &start,
            &end,
            "api",
        ))
    } else {
        None
    };

    let (kind, label) = if weekly_from_api.is_some() && monthly.is_none() {
        ("weekly".into(), "Weekly".into())
    } else {
        ("monthly".into(), "Monthly".into())
    };

    Ok(QuotaInfo {
        used,
        monthly_limit,
        on_demand_cap,
        billing_period_start: start,
        billing_period_end: end.clone(),
        percent_used: percent(used, monthly_limit),
        fetched_at: Utc::now().to_rfc3339(),
        period_kind: kind,
        period_label: label,
        days_until_reset: parse_rfc3339(&end).map(|e| days_until(&e)).unwrap_or(0),
        resets_at: end,
        monthly,
        weekly: weekly_from_api,
    })
}

/// Attach locally tracked weekly usage when API does not provide weekly numbers.
pub fn attach_weekly_tracker(user_id: &str, mut quota: QuotaInfo) -> AppResult<QuotaInfo> {
    // If API already gave weekly, keep it
    if quota.weekly.as_ref().map(|w| w.source.as_str()) == Some("api") {
        return Ok(quota);
    }

    let used = quota.used;
    let monthly_limit = quota.monthly_limit;
    let period_days = parse_rfc3339(&quota.billing_period_start)
        .and_then(|s| {
            parse_rfc3339(&quota.billing_period_end).map(|e| (e - s).num_days().abs().max(1))
        })
        .unwrap_or(30);

    // Prorated weekly allowance from monthly limit
    let weekly_limit = (monthly_limit * 7.0 / period_days as f64).round().max(1.0);

    let now = Utc::now();
    let (week_key, week_start, week_end) = iso_week_bounds(now);

    let mut meta = load_meta()?;
    let tracker = meta.accounts.get(user_id).and_then(|a| a.week_tracker.clone());

    let (used_at_start, tracker_out) = match tracker {
        Some(t) if t.week_key == week_key => {
            // Same week: keep baseline; if used went down (period reset mid-week), re-baseline
            let baseline = if used < t.used_at_week_start {
                used
            } else {
                t.used_at_week_start
            };
            (
                baseline,
                WeekTracker {
                    week_key: week_key.clone(),
                    used_at_week_start: baseline,
                    week_start: week_start.to_rfc3339(),
                    week_end: week_end.to_rfc3339(),
                },
            )
        }
        _ => {
            // New week (or first track): baseline = current used
            (
                used,
                WeekTracker {
                    week_key: week_key.clone(),
                    used_at_week_start: used,
                    week_start: week_start.to_rfc3339(),
                    week_end: week_end.to_rfc3339(),
                },
            )
        }
    };

    if let Some(acc) = meta.accounts.get_mut(user_id) {
        acc.week_tracker = Some(tracker_out.clone());
        save_meta(&meta)?;
    }

    let weekly_used = (used - used_at_start).max(0.0);
    quota.weekly = Some(period_quota(
        "weekly",
        "Weekly",
        weekly_used,
        weekly_limit,
        &week_start.to_rfc3339(),
        &week_end.to_rfc3339(),
        "tracked",
    ));

    // Ensure monthly is filled
    if quota.monthly.is_none() {
        quota.monthly = Some(period_quota(
            "monthly",
            "Monthly",
            used,
            monthly_limit,
            &quota.billing_period_start,
            &quota.billing_period_end,
            "api",
        ));
    }

    Ok(quota)
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
    let mut quota = fetch_quota_for_token(&token)?;
    quota = attach_weekly_tracker(user_id, quota)?;

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

