use crate::agy_cli::probe_agy;
use crate::cache::load_snapshot_cache;
use crate::config::{AppConfig, AppPaths};
use crate::database::HistoryDatabase;
use crate::local_logs::inspect_local_metadata;
use crate::models::{AuthState, CheckSeverity, ConnectivityState, DoctorCheck, DoctorReport};
use crate::utils::{display_email, format_tokens_exact, resolve_executable};

pub fn run_doctor(cfg: &AppConfig, paths: &AppPaths) -> DoctorReport {
    let mut checks = Vec::new();

    // 1. Check `agy` binary + version + live connectivity
    let probe = probe_agy(&cfg.general.agy_binary, cfg.general.cli_timeout_secs);
    match probe.connectivity {
        ConnectivityState::Online => {
            checks.push(DoctorCheck {
                name: "Antigravity CLI (`agy`)".to_string(),
                status: CheckSeverity::Ok,
                detail: format!(
                    "Installed (version {}) and reachable; `agy models` returned {} models.",
                    probe.version.as_deref().unwrap_or("unknown"),
                    probe.models.len()
                ),
            });
        }
        ConnectivityState::CliMissing => {
            checks.push(DoctorCheck {
                name: "Antigravity CLI (`agy`)".to_string(),
                status: CheckSeverity::Error,
                detail: probe
                    .error
                    .unwrap_or_else(|| "Binary `agy` not found in PATH".to_string()),
            });
        }
        ConnectivityState::Offline | ConnectivityState::Error | ConnectivityState::CachedOnly => {
            checks.push(DoctorCheck {
                name: "Antigravity CLI (`agy`)".to_string(),
                status: CheckSeverity::Warn,
                detail: format!(
                    "Version: {}. Live `agy models` check failed: {}",
                    probe.version.as_deref().unwrap_or("unknown"),
                    probe.error.unwrap_or_else(|| "unknown error".to_string())
                ),
            });
        }
    }

    // 2. Check Antigravity local dir & authentication status (without reading token file contents)
    if paths.antigravity_dir.is_dir() {
        let local_meta = inspect_local_metadata(&paths.antigravity_dir);
        let acct = display_email(local_meta.account.email.as_deref(), cfg.general.mask_email);
        let auth_status = if probe.connectivity == ConnectivityState::Online
            && matches!(
                local_meta.account.auth_state,
                AuthState::Expired | AuthState::Unknown
            ) {
            AuthState::Authenticated
        } else {
            local_meta.account.auth_state
        };

        let sev = match auth_status {
            AuthState::Authenticated => CheckSeverity::Ok,
            AuthState::Unknown => CheckSeverity::Warn,
            AuthState::Expired | AuthState::NotLoggedIn | AuthState::VerificationRequired => {
                CheckSeverity::Error
            }
        };
        checks.push(DoctorCheck {
            name: "Authentication".to_string(),
            status: sev,
            detail: format!(
                "State: {} | Account: {} | OAuth artifact present: {}",
                auth_status.as_label(),
                acct,
                local_meta.account.oauth_file_present
            ),
        });
    } else {
        checks.push(DoctorCheck {
            name: "Antigravity Data Dir".to_string(),
            status: CheckSeverity::Error,
            detail: format!("Directory not found: {}", paths.antigravity_dir.display()),
        });
    }

    // 3. Local transcripts & SQLite conversation metadata
    let brain_dir = paths.antigravity_dir.join("brain");
    let conv_dir = paths.antigravity_dir.join("conversations");
    let transcript_count = count_transcripts(&brain_dir);
    let conv_db_count = count_db_files(&conv_dir);
    checks.push(DoctorCheck {
        name: "Local Usage Records".to_string(),
        status: if transcript_count > 0 {
            CheckSeverity::Ok
        } else {
            CheckSeverity::Warn
        },
        detail: format!(
            "{} conversation transcript(s) and {} trajectory DB(s) found under {}",
            format_tokens_exact(transcript_count),
            format_tokens_exact(conv_db_count),
            paths.antigravity_dir.display()
        ),
    });

    // 4. XDG paths, cache, and history DB
    let dir_ok = paths.ensure_runtime_dirs().is_ok();
    let cache_present = load_snapshot_cache(&paths.snapshot_cache).is_some();
    let db_msg = match HistoryDatabase::open(&paths.history_db) {
        Ok(db) => {
            let count = db.snapshot_count().unwrap_or(0);
            format!(
                "Readable/writable ({} stored snapshots) at {}",
                count,
                paths.history_db.display()
            )
        }
        Err(e) => format!("Failed to open {}: {e}", paths.history_db.display()),
    };
    checks.push(DoctorCheck {
        name: "XDG Cache & SQLite History".to_string(),
        status: if dir_ok && !db_msg.starts_with("Failed") {
            CheckSeverity::Ok
        } else {
            CheckSeverity::Error
        },
        detail: format!(
            "Cache present: {} ({}) | History DB: {}",
            cache_present,
            paths.snapshot_cache.display(),
            db_msg
        ),
    });

    // 5. Desktop integration tools (`waybar`, `kitty`)
    let waybar_ok = command_exists("waybar");
    let kitty_ok = command_exists("kitty");
    checks.push(DoctorCheck {
        name: "Wayland / Desktop Binaries".to_string(),
        status: if waybar_ok && kitty_ok {
            CheckSeverity::Ok
        } else {
            CheckSeverity::Warn
        },
        detail: format!(
            "waybar: {} | kitty: {}",
            if waybar_ok { "found" } else { "missing" },
            if kitty_ok { "found" } else { "missing" }
        ),
    });

    // 6. Quota & /usage capability status
    let cache_quota_msg = load_snapshot_cache(&paths.snapshot_cache)
        .filter(|s| s.quota.remote_available && !s.quota.pools.is_empty())
        .map(|s| {
            format!(
                "PTY `/usage` bridge enabled ({} pools cached; plan: {}).",
                s.quota.pools.len(),
                s.quota.plan_tier.as_deref().unwrap_or("Unknown")
            )
        })
        .unwrap_or_else(|| {
            if cfg.general.fetch_tui_usage {
                "PTY `/usage` bridge enabled (run `agmonitor refresh` to populate pool quotas)."
                    .to_string()
            } else {
                "PTY `/usage` bridge disabled (`fetch_tui_usage = false`).".to_string()
            }
        });
    checks.push(DoctorCheck {
        name: "Remote Quota Interface".to_string(),
        status: CheckSeverity::Ok,
        detail: cache_quota_msg,
    });

    let has_error = checks.iter().any(|c| c.status == CheckSeverity::Error);
    let has_warn = checks.iter().any(|c| c.status == CheckSeverity::Warn);
    let overall_exit_code = if has_error {
        2
    } else if has_warn {
        1
    } else {
        0
    };

    DoctorReport {
        checks,
        overall_exit_code,
    }
}

fn count_transcripts(brain_dir: &std::path::Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(brain_dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|e| {
            e.path()
                .join(".system_generated")
                .join("logs")
                .join("transcript.jsonl")
                .is_file()
        })
        .count() as u64
}

fn count_db_files(conv_dir: &std::path::Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(conv_dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("db"))
        .count() as u64
}

fn command_exists(name: &str) -> bool {
    resolve_executable(name).is_some()
}
