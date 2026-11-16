use crate::config::AppConfig;
use crate::models::{AuthState, ConnectivityState, UsageSnapshot, WaybarPayload};
use crate::utils::{
    display_email, escape_pango, format_age, format_duration_secs, format_local_time,
    format_tokens, format_tokens_exact, shorten_model_label,
};
use chrono::Utc;

pub fn build_waybar_payload(snapshot: &UsageSnapshot, cfg: &AppConfig) -> WaybarPayload {
    let now = Utc::now();
    let mut classes = Vec::new();
    let state = classify_state(snapshot, cfg, now);
    classes.push(state.to_string());
    if snapshot.stale {
        classes.push("stale".to_string());
    }

    let model_short = snapshot
        .active_model_label
        .as_deref()
        .map(shorten_model_label)
        .unwrap_or_else(|| "No Model".to_string());

    let text = match state {
        "expired" => {
            let reason = match snapshot.account.auth_state {
                AuthState::NotLoggedIn => "Logged Out",
                AuthState::VerificationRequired => "Verify Acct",
                _ => "Auth Expired",
            };
            format!("󰚩 AG {reason}")
        }
        "unavailable" => "󰚩 AG N/A".to_string(),
        _ => {
            let base = if cfg.waybar.display_mode == "pools" && snapshot.quota.remote_available {
                format_pools_bar(&snapshot.quota)
            } else if matches!(cfg.waybar.display_mode.as_str(), "quota" | "tokens_5h")
                && snapshot.quota.remote_available
                && snapshot.quota.active_pool_remaining_pct.is_some()
            {
                let pct = snapshot.quota.active_pool_remaining_pct.unwrap_or(0.0);
                let reset_part = match snapshot.quota.active_pool_reset_text.as_deref() {
                    Some("Quota available") | None => "Ready".to_string(),
                    Some(r) => r.to_string(),
                };
                format!("󰚩 AG {pct:.0}% · {reset_part}")
            } else if let Some(ref b) = snapshot.quota.local_budget {
                format!("󰚩 AG {:.0}% · {model_short}", b.remaining_pct)
            } else {
                match cfg.waybar.display_mode.as_str() {
                    "tokens_today" => format!(
                        "󰚩 AG {} · {model_short}",
                        format_tokens(snapshot.window_today.fresh_total_tokens)
                    ),
                    "model" => {
                        let avail_cnt = snapshot.models.iter().filter(|m| m.available).count();
                        format!("󰚩 {model_short} ({avail_cnt})")
                    }
                    "compact" => format!(
                        "󰚩 AG {}",
                        format_tokens(snapshot.window_5h.fresh_total_tokens)
                    ),
                    _ => format!(
                        "󰚩 AG 5h {} · {model_short}",
                        format_tokens(snapshot.window_5h.fresh_total_tokens)
                    ),
                }
            };

            if snapshot.connectivity == ConnectivityState::Offline
                || snapshot.connectivity == ConnectivityState::Error
            {
                format!("{base} ⚠")
            } else if snapshot.stale {
                format!("{base} ~")
            } else {
                base
            }
        }
    };

    let percentage = snapshot
        .quota
        .active_pool_remaining_pct
        .or_else(|| {
            snapshot
                .quota
                .local_budget
                .as_ref()
                .map(|b| b.remaining_pct)
        })
        .map(|p| p.round().clamp(0.0, 100.0) as u64);

    let tooltip = build_tooltip(snapshot, cfg, now);

    WaybarPayload {
        text,
        tooltip,
        class: classes,
        alt: state.to_string(),
        percentage,
    }
}

fn format_pools_bar(quota: &crate::models::QuotaInfo) -> String {
    let mut parts = Vec::new();
    for p in &quota.pools {
        let short = if p.pool_name.contains("ARGON") {
            "A"
        } else if p.pool_name.contains("CLAUDE") {
            "C"
        } else {
            "G"
        };
        if let Some(lim) = p.limiting_window() {
            parts.push(format!("{short}:{:.0}%", lim.remaining_pct));
        }
    }
    if parts.is_empty() {
        "󰚩 AG --".to_string()
    } else {
        format!("󰚩 {}", parts.join(" "))
    }
}

pub fn classify_state(
    snapshot: &UsageSnapshot,
    cfg: &AppConfig,
    now: chrono::DateTime<Utc>,
) -> &'static str {
    match snapshot.account.auth_state {
        AuthState::Expired | AuthState::NotLoggedIn | AuthState::VerificationRequired => {
            return "expired";
        }
        _ => {}
    }

    if snapshot.connectivity == ConnectivityState::CliMissing
        && snapshot.models.is_empty()
        && snapshot.window_all_time.planner_steps == 0
    {
        return "unavailable";
    }

    if let Some(rem) = snapshot.quota.active_pool_remaining_pct {
        if rem < cfg.waybar.critical_remaining_pct {
            return "critical";
        }
        if rem < cfg.waybar.warning_remaining_pct {
            return "warning";
        }
    } else if let Some(ref b) = snapshot.quota.local_budget {
        if b.remaining_pct < cfg.waybar.critical_remaining_pct {
            return "critical";
        }
        if b.remaining_pct < cfg.waybar.warning_remaining_pct {
            return "warning";
        }
    }

    let expiring_soon = snapshot
        .account
        .token_expiry
        .map(|exp| (exp - now).num_seconds() < 600)
        .unwrap_or(false);

    if snapshot.stale
        || snapshot.connectivity == ConnectivityState::Offline
        || snapshot.connectivity == ConnectivityState::Error
        || snapshot.connectivity == ConnectivityState::CliMissing
        || expiring_soon
    {
        return "warning";
    }

    "normal"
}

fn build_tooltip(snapshot: &UsageSnapshot, cfg: &AppConfig, now: chrono::DateTime<Utc>) -> String {
    let ver = snapshot
        .agy_version
        .as_deref()
        .map(|v| format!(" (agy {v})"))
        .unwrap_or_default();
    let acct = display_email(snapshot.account.email.as_deref(), cfg.general.mask_email);
    let tier = snapshot
        .quota
        .plan_tier
        .as_deref()
        .map(|t| format!(" · {t}"))
        .unwrap_or_default();
    let auth = snapshot.account.auth_state.as_label();
    let conn = snapshot.connectivity.as_label();
    let active_model = snapshot
        .active_model_label
        .as_deref()
        .unwrap_or("Unavailable");
    let avail_models = snapshot.models.iter().filter(|m| m.available).count();

    let expiry_line = match snapshot.account.token_expiry {
        Some(exp) => {
            let diff = (exp - now).num_seconds();
            if diff > 0 {
                format!(
                    "{} (in {})",
                    format_local_time(exp),
                    format_duration_secs(diff)
                )
            } else {
                format!("{} (auto-refreshed by agy)", format_local_time(exp))
            }
        }
        None => "Unavailable".to_string(),
    };

    let last_live = snapshot
        .last_live_refresh_at
        .map(|t| format!("{} ({})", format_local_time(t), format_age(t, now)))
        .unwrap_or_else(|| "Never".to_string());

    let mut lines = vec![
        format!("<b>Antigravity Usage Monitor{}</b>", escape_pango(&ver)),
        format!(
            "Account: <b>{}{}</b>  [{}]",
            escape_pango(&acct),
            escape_pango(&tier),
            escape_pango(auth)
        ),
        format!(
            "Status: {}  ·  Models: {} available",
            escape_pango(conn),
            avail_models
        ),
        format!("Active Model: <b>{}</b>", escape_pango(active_model)),
        format!("OAuth Access Expiry: {}", escape_pango(&expiry_line)),
        String::new(),
    ];

    if snapshot.quota.remote_available && !snapshot.quota.pools.is_empty() {
        lines.push("<b>Remaining Quota (/usage)</b>".to_string());
        for p in &snapshot.quota.pools {
            let mut parts = Vec::new();
            if let Some(ref fh) = p.five_hour {
                let rst = fh
                    .refresh_in_text
                    .as_deref()
                    .filter(|s| *s != "Quota available")
                    .map(|s| format!(" ({s})"))
                    .unwrap_or_default();
                parts.push(format!("5h: {:.1}%{rst}", fh.remaining_pct));
            }
            if let Some(ref wk) = p.weekly {
                let rst = wk
                    .refresh_in_text
                    .as_deref()
                    .filter(|s| *s != "Quota available")
                    .map(|s| format!(" ({s})"))
                    .unwrap_or_default();
                parts.push(format!("Wk: {:.1}%{rst}", wk.remaining_pct));
            }
            lines.push(format!(
                "<tt>{:<22} {}</tt>",
                escape_pango(&p.pool_name),
                escape_pango(&parts.join(" · "))
            ));
        }
    } else {
        lines.push(format!(
            "Remote Quota: Unavailable ({})",
            escape_pango(&snapshot.quota.unavailability_reason)
        ));
    }

    if let Some(ref b) = snapshot.quota.local_budget {
        lines.push(format!(
            "Local 5h Budget: {:.1}% left ({} / {})",
            b.remaining_pct,
            format_tokens(b.used_tokens),
            format_tokens(b.budget_tokens)
        ));
    }

    lines.push(String::new());
    lines.push("<b>Actual Local Token Usage (Transcripts)</b>".to_string());
    lines.push(format!(
        "<tt>5h    : {:>8} fresh  ({} in · {} out · {} cache)</tt>",
        format_tokens(snapshot.window_5h.fresh_total_tokens),
        format_tokens(snapshot.window_5h.input_tokens),
        format_tokens(snapshot.window_5h.output_tokens),
        format_tokens(snapshot.window_5h.cache_read_tokens),
    ));
    lines.push(format!(
        "<tt>Today : {:>8} fresh  ({} in · {} out · {} cache)</tt>",
        format_tokens(snapshot.window_today.fresh_total_tokens),
        format_tokens(snapshot.window_today.input_tokens),
        format_tokens(snapshot.window_today.output_tokens),
        format_tokens(snapshot.window_today.cache_read_tokens),
    ));
    lines.push(format!(
        "<tt>7d    : {:>8} fresh  ({} steps)</tt>",
        format_tokens(snapshot.window_7d.fresh_total_tokens),
        format_tokens_exact(snapshot.window_7d.planner_steps),
    ));
    lines.push(format!(
        "<tt>All   : {:>8} fresh  ({} combined)</tt>",
        format_tokens(snapshot.window_all_time.fresh_total_tokens),
        format_tokens(snapshot.window_all_time.combined_total_tokens),
    ));

    if let Some(ref sl) = snapshot.active_statusline {
        let pct = sl
            .context_used_pct
            .map(|p| format!("{p:.0}%"))
            .unwrap_or_else(|| "N/A".to_string());
        lines.push(format!(
            "Live Session Context: {} ({})",
            escape_pango(&pct),
            escape_pango(sl.agent_state.as_deref().unwrap_or("active"))
        ));
    }

    if snapshot.stale {
        if let Some(ref reason) = snapshot.stale_reason {
            lines.push(format!("<i>Stale: {}</i>", escape_pango(reason)));
        } else {
            lines.push("<i>Stale: cached snapshot</i>".to_string());
        }
    }

    lines.push(String::new());
    lines.push(format!("Last Live Check: {}", escape_pango(&last_live)));
    lines.push("<i>Left: TUI  ·  Right: Refresh  ·  Middle: Open agy (/usage)</i>".to_string());

    lines.join("\n")
}
