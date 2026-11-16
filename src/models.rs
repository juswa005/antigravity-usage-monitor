use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthState {
    Authenticated,
    Expired,
    NotLoggedIn,
    VerificationRequired,
    Unknown,
}

impl AuthState {
    pub fn as_label(self) -> &'static str {
        match self {
            Self::Authenticated => "Authenticated",
            Self::Expired => "Expired",
            Self::NotLoggedIn => "Not Logged In",
            Self::VerificationRequired => "Verification Required",
            Self::Unknown => "Unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectivityState {
    Online,
    Offline,
    CliMissing,
    Error,
    CachedOnly,
}

impl ConnectivityState {
    pub fn as_label(self) -> &'static str {
        match self {
            Self::Online => "Online",
            Self::Offline => "Offline",
            Self::CliMissing => "CLI Missing",
            Self::Error => "Error",
            Self::CachedOnly => "Cached",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountInfo {
    pub email: Option<String>,
    pub auth_method: Option<String>,
    pub auth_state: AuthState,
    pub oauth_file_present: bool,
    pub oauth_file_mtime: Option<DateTime<Utc>>,
    pub token_expiry: Option<DateTime<Utc>>,
    pub last_auth_error: Option<String>,
}

impl Default for AccountInfo {
    fn default() -> Self {
        Self {
            email: None,
            auth_method: None,
            auth_state: AuthState::Unknown,
            oauth_file_present: false,
            oauth_file_mtime: None,
            token_expiry: None,
            last_auth_error: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaWindow {
    pub window_label: String,
    pub remaining_pct: f64,
    pub used_pct: f64,
    pub refresh_in_text: Option<String>,
    pub refresh_in_secs: Option<i64>,
    pub refreshes_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaPool {
    pub pool_name: String,
    pub models_included: Vec<String>,
    pub five_hour: Option<QuotaWindow>,
    pub weekly: Option<QuotaWindow>,
}

impl QuotaPool {
    pub fn limiting_window(&self) -> Option<&QuotaWindow> {
        match (&self.five_hour, &self.weekly) {
            (Some(fh), Some(wk)) => {
                if fh.remaining_pct <= wk.remaining_pct {
                    Some(fh)
                } else {
                    Some(wk)
                }
            }
            (Some(fh), None) => Some(fh),
            (None, Some(wk)) => Some(wk),
            (None, None) => None,
        }
    }

    pub fn matches_model(&self, model_label_or_id: &str) -> bool {
        let low = model_label_or_id.to_lowercase();
        if self
            .models_included
            .iter()
            .any(|inc| low.contains(&inc.to_lowercase()))
        {
            return true;
        }
        let pool_low = self.pool_name.to_lowercase();
        if pool_low.contains("argon") {
            return low.contains("argon");
        }
        if pool_low.contains("claude") || pool_low.contains("gpt") {
            return low.contains("claude") || low.contains("gpt");
        }
        if pool_low.contains("gemini") {
            return (low.contains("gemini") || low.contains("hydrogen") || low.contains("flash"))
                && !low.contains("argon");
        }
        false
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalBudgetStatus {
    pub budget_tokens: u64,
    pub used_tokens: u64,
    pub remaining_tokens: u64,
    pub used_pct: f64,
    pub remaining_pct: f64,
    pub include_cache_read: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaInfo {
    pub remote_available: bool,
    pub used_pct: Option<f64>,
    pub remaining_pct: Option<f64>,
    pub reset_at: Option<DateTime<Utc>>,
    pub reset_countdown_secs: Option<i64>,
    pub reset_in_text: Option<String>,
    pub plan_tier: Option<String>,
    pub ai_credits_remaining: Option<f64>,
    pub remaining_token_allowance: Option<u64>,
    pub pools: Vec<QuotaPool>,
    pub active_pool_name: Option<String>,
    pub active_pool_remaining_pct: Option<f64>,
    pub active_pool_reset_text: Option<String>,
    pub overall_lowest_pct: Option<f64>,
    pub overall_avg_pct: Option<f64>,
    pub unavailability_reason: String,
    pub local_budget: Option<LocalBudgetStatus>,
}

impl Default for QuotaInfo {
    fn default() -> Self {
        Self {
            remote_available: false,
            used_pct: None,
            remaining_pct: None,
            reset_at: None,
            reset_countdown_secs: None,
            reset_in_text: None,
            plan_tier: None,
            ai_credits_remaining: None,
            remaining_token_allowance: None,
            pools: Vec::new(),
            active_pool_name: None,
            active_pool_remaining_pct: None,
            active_pool_reset_text: None,
            overall_lowest_pct: None,
            overall_avg_pct: None,
            unavailability_reason: "Remote quota unavailable (offline or `agy /usage` timed out)."
                .to_string(),
            local_budget: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: String,
    pub family: String,
    pub available: bool,
    pub is_active: bool,
    pub steps_all_time: u64,
    pub steps_5h: u64,
    pub input_tokens: u64,
    pub cache_read_tokens: u64,
    pub output_tokens: u64,
    pub thinking_tokens: u64,
    pub response_tokens: u64,
    pub remote_quota_pct: Option<f64>,
    #[serde(default)]
    pub five_hour_pct: Option<f64>,
    #[serde(default)]
    pub weekly_pct: Option<f64>,
    #[serde(default)]
    pub reset_in_text: Option<String>,
    #[serde(default)]
    pub pool_name: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenWindowStats {
    pub window_name: String,
    pub input_tokens: u64,
    pub cache_read_tokens: u64,
    pub output_tokens: u64,
    pub thinking_tokens: u64,
    pub response_tokens: u64,
    pub fresh_total_tokens: u64,
    pub combined_total_tokens: u64,
    pub planner_steps: u64,
    pub error_steps: u64,
    pub conversations_active: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyTokenBucket {
    pub day_local: String,
    pub planner_steps: u64,
    pub input_tokens: u64,
    pub cache_read_tokens: u64,
    pub output_tokens: u64,
    pub thinking_tokens: u64,
    pub response_tokens: u64,
    pub conversations_active: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationUsage {
    pub conversation_id: String,
    pub title: String,
    pub workspace: String,
    pub primary_model: Option<String>,
    pub last_activity: Option<DateTime<Utc>>,
    pub planner_steps: u64,
    pub input_tokens: u64,
    pub cache_read_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveStatusLine {
    pub recorded_at: DateTime<Utc>,
    pub agent_state: Option<String>,
    pub model_display_name: Option<String>,
    pub context_used_pct: Option<f64>,
    pub conversation_title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageSnapshot {
    pub collected_at: DateTime<Utc>,
    pub last_live_refresh_at: Option<DateTime<Utc>>,
    pub agy_version: Option<String>,
    pub connectivity: ConnectivityState,
    pub stale: bool,
    pub stale_reason: Option<String>,
    pub collection_error: Option<String>,
    pub account: AccountInfo,
    pub active_model_label: Option<String>,
    pub quota: QuotaInfo,
    pub models: Vec<ModelInfo>,
    pub window_5h: TokenWindowStats,
    pub window_today: TokenWindowStats,
    pub window_7d: TokenWindowStats,
    pub window_30d: TokenWindowStats,
    pub window_all_time: TokenWindowStats,
    pub daily_buckets: Vec<DailyTokenBucket>,
    pub top_conversations: Vec<ConversationUsage>,
    pub active_statusline: Option<ActiveStatusLine>,
    pub sources_checked: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotHistoryRow {
    pub id: i64,
    pub collected_at: String,
    pub auth_state: String,
    pub connectivity: String,
    pub active_model: Option<String>,
    pub available_models_count: u64,
    pub tokens_5h_fresh: u64,
    pub tokens_5h_combined: u64,
    pub tokens_today_fresh: u64,
    pub tokens_all_combined: u64,
    #[serde(default)]
    pub remote_quota_pct: Option<f64>,
    pub collection_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeeklySummaryRow {
    pub iso_week: String,
    pub start_day: String,
    pub end_day: String,
    pub active_days: u64,
    pub planner_steps: u64,
    pub input_tokens: u64,
    pub cache_read_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaybarPayload {
    pub text: String,
    pub tooltip: String,
    pub class: Vec<String>,
    pub alt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percentage: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckSeverity {
    Ok,
    Warn,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorCheck {
    pub name: String,
    pub status: CheckSeverity,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorReport {
    pub checks: Vec<DoctorCheck>,
    pub overall_exit_code: i32,
}
