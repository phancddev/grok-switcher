use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaFile {
    #[serde(default)]
    pub accounts: HashMap<String, AccountMeta>,
    #[serde(default)]
    pub active_user_id: Option<String>,
}

/// One entry inside Grok's auth.json map.
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
