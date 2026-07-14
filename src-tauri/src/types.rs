use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// One progress period (week or month).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeriodQuota {
    pub kind: String,  // "weekly" | "monthly"
    pub label: String, // "Weekly" | "Monthly"
    pub used: f64,
    pub limit: f64,
    pub percent_used: f64,
    pub period_start: String,
    pub period_end: String,
    pub resets_at: String,
    pub days_until_reset: i64,
    /// "api" = from Grok billing; "tracked" = local week tracking + prorated limit
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaInfo {
    pub used: f64,
    pub monthly_limit: f64,
    pub on_demand_cap: f64,
    pub billing_period_start: String,
    pub billing_period_end: String,
    pub percent_used: f64,
    pub fetched_at: String,
    #[serde(default)]
    pub period_kind: String,
    #[serde(default)]
    pub period_label: String,
    #[serde(default)]
    pub days_until_reset: i64,
    #[serde(default)]
    pub resets_at: String,
    /// Monthly period (always from API when available)
    #[serde(default)]
    pub monthly: Option<PeriodQuota>,
    /// Weekly period (API if present, else locally tracked)
    #[serde(default)]
    pub weekly: Option<PeriodQuota>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSummary {
    pub user_id: String,
    pub email: String,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub label: Option<String>,
    pub is_active: bool,
    pub last_used: Option<String>,
    pub created_at: Option<String>,
    pub quota: Option<QuotaInfo>,
    pub tier: Option<i64>,
    pub subscription_tier: Option<String>,
    pub plan_expires_at: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WeekTracker {
    /// ISO week key e.g. "2026-W28"
    pub week_key: String,
    /// `used` credits snapshot at the start of this ISO week
    pub used_at_week_start: f64,
    pub week_start: String,
    pub week_end: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountMeta {
    pub email: String,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub label: Option<String>,
    pub last_used: Option<String>,
    pub created_at: Option<String>,
    pub quota: Option<QuotaInfo>,
    pub tier: Option<i64>,
    #[serde(default)]
    pub subscription_tier: Option<String>,
    #[serde(default)]
    pub plan_expires_at: Option<String>,
    /// Local tracker for weekly usage (API does not expose weekly for GrokPro)
    #[serde(default)]
    pub week_tracker: Option<WeekTracker>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaFile {
    #[serde(default)]
    pub accounts: HashMap<String, AccountMeta>,
    #[serde(default)]
    pub active_user_id: Option<String>,
    /// Account IDs whose name and email are visually hidden in the app.
    #[serde(default)]
    pub masked_account_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthEntry {
    pub key: String,
    #[serde(default)]
    pub auth_mode: Option<String>,
    #[serde(default)]
    pub create_time: Option<String>,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub first_name: Option<String>,
    #[serde(default)]
    pub last_name: Option<String>,
    #[serde(default)]
    pub principal_type: Option<String>,
    #[serde(default)]
    pub principal_id: Option<String>,
    #[serde(default)]
    pub team_id: Option<String>,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub oidc_issuer: Option<String>,
    #[serde(default)]
    pub oidc_client_id: Option<String>,
    #[serde(default)]
    pub coding_data_retention_opt_out: Option<bool>,
}

pub type AuthFile = HashMap<String, AuthEntry>;
