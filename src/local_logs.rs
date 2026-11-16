use crate::models::{AccountInfo, AuthState};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct LocalMetadata {
    pub account: AccountInfo,
    pub selected_model: Option<String>,
    pub sources_checked: Vec<String>,
}

pub fn inspect_local_metadata(antigravity_dir: &Path) -> LocalMetadata {
    let mut meta = LocalMetadata::default();

    // 1. Check active model in settings.json
    let settings_path = antigravity_dir.join("settings.json");
    if let Ok(raw) = fs::read_to_string(&settings_path) {
        meta.sources_checked
            .push("antigravity-cli/settings.json".to_string());
        if let Ok(json) = serde_json::from_str::<Value>(&raw) {
            if let Some(model) = json.get("model").and_then(|v| v.as_str()) {
                if !model.trim().is_empty() {
                    meta.selected_model = Some(model.trim().to_string());
                }
            }
        }
    }

    // 2. Stat antigravity-oauth-token WITHOUT reading its contents (never touch credentials)
    let oauth_path = antigravity_dir.join("antigravity-oauth-token");
    if let Ok(stat) = fs::metadata(&oauth_path) {
        meta.sources_checked
            .push("antigravity-cli/antigravity-oauth-token (stat only)".to_string());
        meta.account.oauth_file_present = stat.is_file() && stat.len() > 0;
        if let Ok(mtime) = stat.modified() {
            meta.account.oauth_file_mtime = Some(DateTime::<Utc>::from(mtime));
        }
    }

    // 3. Inspect recent cli-*.log files chronologically for account email, auth method, expiry, and auth errors
    let log_dir = antigravity_dir.join("log");
    let mut log_files: Vec<PathBuf> = fs::read_dir(&log_dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok().map(|entry| entry.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("cli-") && n.ends_with(".log"))
                .unwrap_or(false)
        })
        .collect();
    log_files.sort();
    if log_files.len() > 12 {
        let start = log_files.len() - 12;
        log_files = log_files.split_off(start);
    }

    if !log_files.is_empty() {
        meta.sources_checked.push(format!(
            "antigravity-cli/log/*.log ({} files)",
            log_files.len()
        ));
    }

    let mut logged_out = false;
    let mut expired_flag: Option<bool> = None;

    for log_path in &log_files {
        let Ok(content) = fs::read_to_string(log_path) else {
            continue;
        };
        for line in content.lines() {
            if let Some(rest) = line.split("applyAuthResult:").nth(1) {
                if let Some(email) = extract_kv(rest, "email=") {
                    meta.account.email = Some(email);
                }
                if let Some(method) = extract_kv(rest, "authMethod=") {
                    meta.account.auth_method = Some(method);
                }
                logged_out = false;
                meta.account.last_auth_error = None;
            } else if line.contains("Logging out current user") {
                logged_out = true;
                meta.account.email = None;
                meta.account.auth_method = None;
                meta.account.token_expiry = None;
                expired_flag = None;
            } else if line.contains("expiry=") {
                if let Some(dt) = parse_go_expiry(line) {
                    meta.account.token_expiry = Some(dt);
                }
                if let Some(flag) = extract_kv(line, "expired=") {
                    expired_flag = match flag.as_str() {
                        "true" => Some(true),
                        "false" => Some(false),
                        _ => expired_flag,
                    };
                }
            } else if meta.selected_model.is_none() {
                if let Some(idx) =
                    line.find("Propagating selected model override to backend: label=\"")
                {
                    let after = &line
                        [idx + "Propagating selected model override to backend: label=\"".len()..];
                    if let Some((lbl, _)) = after.split_once('"') {
                        if !lbl.is_empty() {
                            meta.selected_model = Some(lbl.to_string());
                        }
                    }
                }
            }

            if line.contains("PERMISSION_DENIED (code 403)") {
                meta.account.last_auth_error = Some(
                    "PERMISSION_DENIED (code 403): Verify your account to continue.".to_string(),
                );
            } else if line.contains("You are not logged into Antigravity") && logged_out {
                meta.account.last_auth_error = Some("Not logged into Antigravity".to_string());
            }
        }
    }

    let now = Utc::now();
    meta.account.auth_state = if logged_out || !meta.account.oauth_file_present {
        AuthState::NotLoggedIn
    } else if meta
        .account
        .last_auth_error
        .as_deref()
        .map(|s| s.contains("Verify your account"))
        .unwrap_or(false)
    {
        AuthState::VerificationRequired
    } else if expired_flag == Some(true)
        || meta
            .account
            .token_expiry
            .map(|exp| exp <= now)
            .unwrap_or(false)
    {
        // Note: agy holds a refresh token in antigravity-oauth-token and refreshes access tokens on demand,
        // so an access-token expiry timestamp in logs indicates stale access token until agy runs.
        AuthState::Expired
    } else if meta.account.email.is_some() || meta.account.oauth_file_present {
        AuthState::Authenticated
    } else {
        AuthState::Unknown
    };

    meta
}

fn extract_kv(text: &str, prefix: &str) -> Option<String> {
    let idx = text.find(prefix)?;
    let val = &text[idx + prefix.len()..];
    let end = val
        .find(|c: char| c == ',' || c.is_whitespace())
        .unwrap_or(val.len());
    let slice = val[..end].trim();
    if slice.is_empty() {
        None
    } else {
        Some(slice.to_string())
    }
}

pub fn parse_go_expiry(line: &str) -> Option<DateTime<Utc>> {
    let idx = line.find("expiry=")?;
    let rest = &line[idx + "expiry=".len()..];
    let parts: Vec<&str> = rest.split_whitespace().collect();
    if parts.len() < 3 {
        return None;
    }
    let ts_str = format!("{} {} {}", parts[0], parts[1], parts[2]);
    DateTime::parse_from_str(&ts_str, "%Y-%m-%d %H:%M:%S%.f %z")
        .or_else(|_| DateTime::parse_from_str(&ts_str, "%Y-%m-%d %H:%M:%S %z"))
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}
