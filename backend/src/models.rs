use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct NodePool {
    pub id: String,
    pub name: String,
    pub source_type: String,
    pub subscription_url: Option<String>,
    pub data_format: String,
    pub encoding_format: String,
    pub format_config: Option<String>,
    pub charset: String,
    pub default_protocol: String,
    pub update_interval: i64,
    pub detection_policy_id: Option<String>,
    pub enabled: i64,
    pub last_sync: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PoolPayload {
    pub name: String,
    pub source_type: String,
    pub subscription_url: Option<String>,
    pub data_format: Option<String>,
    pub encoding_format: Option<String>,
    pub format_config: Option<serde_json::Value>,
    pub charset: Option<String>,
    pub default_protocol: Option<String>,
    pub update_interval: Option<i64>,
    pub detection_policy_id: Option<String>,
    pub enabled: Option<bool>,
    pub local_data: Option<String>,
    pub import_mode: Option<String>,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct ProxyNode {
    pub id: String,
    pub pool_id: String,
    pub protocol: String,
    pub host: String,
    pub port: i64,
    pub username: Option<String>,
    pub password: Option<String>,
    pub raw: Option<String>,
    pub exit_ip: Option<String>,
    pub country: Option<String>,
    pub country_code: Option<String>,
    pub asn: Option<i64>,
    pub asn_org: Option<String>,
    pub is_residential: i64,
    pub latency_ms: Option<i64>,
    pub fail_count: i64,
    pub status: String,
    pub isolated_until: Option<String>,
    pub last_check: Option<String>,
    pub last_used: Option<String>,
    pub next_check: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DetectionPolicy {
    pub id: String,
    pub name: String,
    pub check_interval: i64,
    pub check_url: String,
    pub timeout_ms: i64,
    pub on_fail: String,
    pub isolate_seconds: i64,
    pub backoff_base_seconds: i64,
    pub max_fails: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PolicyPayload {
    pub name: String,
    pub check_interval: Option<i64>,
    pub check_url: Option<String>,
    pub timeout_ms: Option<i64>,
    pub on_fail: Option<String>,
    pub isolate_seconds: Option<i64>,
    pub backoff_base_seconds: Option<i64>,
    pub max_fails: Option<i64>,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct ServiceNode {
    pub id: String,
    pub name: String,
    pub listen_host: String,
    pub listen_port: i64,
    pub enable_http: i64,
    pub enable_socks5: i64,
    pub enable_socks5h: i64,
    pub username: String,
    pub password: String,
    pub pool_ids: String,
    pub filter_countries: String,
    pub filter_asns: String,
    pub filter_residential: i64,
    pub selection_strategy: String,
    pub sticky_ttl: i64,
    pub enabled: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServicePayload {
    pub name: String,
    pub listen_host: Option<String>,
    pub listen_port: i64,
    pub enable_http: Option<bool>,
    pub enable_socks5: Option<bool>,
    pub enable_socks5h: Option<bool>,
    pub username: String,
    pub password: String,
    pub pool_ids: Option<Vec<String>>,
    pub filter_countries: Option<Vec<String>>,
    pub filter_asns: Option<Vec<i64>>,
    pub filter_residential: Option<bool>,
    pub selection_strategy: Option<String>,
    pub sticky_ttl: Option<i64>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct LoginReq {
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct SettingsUpdate {
    pub server_host: Option<String>,
    pub old_password: Option<String>,
    pub new_password: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ParsedProxy {
    pub protocol: String,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    pub raw: String,
}
