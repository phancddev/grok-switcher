use crate::auth::{jwt_tier, primary_entry};
use crate::error::{AppError, AppResult};
use crate::store::{get_access_token, load_account_snapshot, update_quota};
use crate::types::QuotaInfo;
use serde::Deserialize;

const BILLING_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing";
const USER_URL: &str = "https://cli-chat-proxy.grok.com/v1/user";

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
    billing_period_start: String,
    billing_period_end: String,
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
    let used = data.config.used.val;
    let monthly_limit = data.config.monthly_limit.val;
    let on_demand_cap = data
        .config
        .on_demand_cap
        .map(|v| v.val)
        .unwrap_or(0.0);
    let percent_used = if monthly_limit > 0.0 {
        (used / monthly_limit) * 100.0
    } else {
        0.0
    };

    Ok(QuotaInfo {
        used,
        monthly_limit,
        on_demand_cap,
        billing_period_start: data.config.billing_period_start,
        billing_period_end: data.config.billing_period_end,
        percent_used,
        fetched_at: chrono::Utc::now().to_rfc3339(),
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

pub fn refresh_quota(user_id: &str) -> AppResult<QuotaInfo> {
    let token = get_access_token(user_id)?;
    let quota = fetch_quota_for_token(&token)?;
    let auth = load_account_snapshot(user_id)?;
    let (_, entry) = primary_entry(&auth)?;
    let tier = jwt_tier(&entry.key);
    update_quota(user_id, quota.clone(), tier)?;
    Ok(quota)
}
