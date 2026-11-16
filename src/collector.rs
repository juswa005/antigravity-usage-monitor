use crate::agy_cli::{probe_agy, AgyModelEntry};
use crate::agy_usage::probe_agy_usage;
use crate::cache::{
    is_live_cache_fresh, load_active_statusline, load_snapshot_cache, save_snapshot_cache,
    try_acquire_refresh_lock,
};
use crate::config::{AppConfig, AppPaths};
use crate::database::HistoryDatabase;
use crate::local_logs::inspect_local_metadata;
use crate::models::{
    AuthState, ConnectivityState, DailyTokenBucket, LocalBudgetStatus, ModelInfo, QuotaInfo,
    QuotaPool, QuotaWindow, UsageSnapshot,
};
use crate::transcripts::{collect_local_transcripts, ModelUsageTotals};
use crate::utils::{format_duration_secs, model_family_from_label};
use chrono::{DateTime, Utc};
use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

#[derive(Debug, Clone, Copy, Default)]
pub struct CollectOptions {
    pub force_refresh: bool,
    pub cache_only: bool,
}

pub fn collect_usage(cfg: &AppConfig, paths: &AppPaths, opts: CollectOptions) -> UsageSnapshot {
    let _ = paths.ensure_runtime_dirs();
    let now = Utc::now();
    let cached = load_snapshot_cache(&paths.snapshot_cache);

    if let Some(ref prev) = cached {
        if !opts.force_refresh
            && (opts.cache_only
                || is_live_cache_fresh(prev, cfg.general.refresh_interval_secs, now))
        {
            if opts.cache_only {
                let mut clone = prev.clone();
                clone.active_statusline = load_active_statusline(&paths.statusline_cache, 1800);
                refresh_quota_countdowns(&mut clone.quota, now);
                return clone;
            }
            return build_snapshot(cfg, paths, Some(prev), None, None, now);
        }
    }

    if opts.cache_only {
        return build_snapshot(cfg, paths, cached.as_ref(), None, None, now);
    }

    let lock_ttl =
        Duration::from_secs(cfg.general.cli_timeout_secs + cfg.general.usage_timeout_secs + 15);
    let _guard = match try_acquire_refresh_lock(&paths.refresh_lock, lock_ttl) {
        Some(g) => g,
        None => {
            return build_snapshot(cfg, paths, cached.as_ref(), None, None, now);
        }
    };

    let probe = probe_agy(&cfg.general.agy_binary, cfg.general.cli_timeout_secs);
    let usage_probe =
        if probe.connectivity == ConnectivityState::Online && cfg.general.fetch_tui_usage {
            Some(probe_agy_usage(
                &cfg.general.agy_binary,
                &paths.antigravity_dir,
                cfg.general.usage_timeout_secs,
            ))
        } else {
            None
        };

    let snap = build_snapshot(cfg, paths, cached.as_ref(), Some(probe), usage_probe, now);

    let _ = save_snapshot_cache(&paths.snapshot_cache, &snap);
    if let Ok(mut db) = HistoryDatabase::open(&paths.history_db) {
        let _ = db.record_snapshot(&snap, cfg.history.max_snapshots, opts.force_refresh);
    }

    snap
}

fn build_snapshot(
    cfg: &AppConfig,
    paths: &AppPaths,
    cached: Option<&UsageSnapshot>,
    live_probe: Option<crate::agy_cli::AgyCliProbe>,
    usage_probe: Option<crate::agy_usage::AgyUsageProbe>,
    now: DateTime<Utc>,
) -> UsageSnapshot {
    let mut local_meta = inspect_local_metadata(&paths.antigravity_dir);
    let transcripts = collect_local_transcripts(&paths.antigravity_dir, now);
    let active_statusline = load_active_statusline(&paths.statusline_cache, 1800);

    let mut sources = local_meta.sources_checked.clone();
    sources.extend(transcripts.sources_checked.clone());
    if active_statusline.is_some() {
        sources.push("agmonitor/active_statusline.json".to_string());
    }

    let active_model_label = active_statusline
        .as_ref()
        .and_then(|s| s.model_display_name.clone())
        .or_else(|| local_meta.selected_model.clone())
        .or_else(|| cached.and_then(|c| c.active_model_label.clone()));

    let mut agy_version = cached.and_then(|c| c.agy_version.clone());
    let mut last_live_refresh_at = cached.and_then(|c| c.last_live_refresh_at);
    let mut connectivity = cached
        .map(|_| ConnectivityState::CachedOnly)
        .unwrap_or(ConnectivityState::Offline);
    let mut stale = false;
    let mut stale_reason: Option<String> = None;
    let mut collection_error: Option<String> = None;

    let live_entries: Vec<AgyModelEntry> = match live_probe {
        Some(probe) => {
            if probe.version.is_some() {
                agy_version = probe.version.clone();
            }
            if probe.connectivity == ConnectivityState::Online {
                sources.push("cli:agy models".to_string());
                last_live_refresh_at = Some(now);
                connectivity = ConnectivityState::Online;
                if local_meta.account.auth_state == AuthState::Expired
                    || local_meta.account.auth_state == AuthState::Unknown
                {
                    local_meta.account.auth_state = AuthState::Authenticated;
                }
                probe.models
            } else {
                connectivity = probe.connectivity;
                collection_error = probe.error.clone();
                if let Some(auth_err) = probe.auth_error {
                    local_meta.account.last_auth_error = Some(auth_err.clone());
                    local_meta.account.auth_state = classify_auth_error(&auth_err);
                }
                stale = true;
                stale_reason = probe
                    .error
                    .clone()
                    .or_else(|| Some("Using cached model availability".to_string()));
                cached
                    .map(|prev| {
                        prev.models
                            .iter()
                            .filter(|m| m.available)
                            .map(|m| AgyModelEntry {
                                id: m.id.clone(),
                                display_name: m.display_name.clone(),
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            }
        }
        None => {
            if let Some(prev) = cached {
                connectivity = prev.connectivity;
                stale = prev.stale
                    || !is_live_cache_fresh(prev, cfg.general.refresh_interval_secs, now);
                stale_reason = prev.stale_reason.clone();
                collection_error = prev.collection_error.clone();
                if prev.connectivity == ConnectivityState::Online {
                    sources.push("cache:agy models".to_string());
                }
                prev.models
                    .iter()
                    .filter(|m| m.available)
                    .map(|m| AgyModelEntry {
                        id: m.id.clone(),
                        display_name: m.display_name.clone(),
                    })
                    .collect()
            } else {
                stale = true;
                stale_reason = Some("No cached `agy models` snapshot yet".to_string());
                Vec::new()
            }
        }
    };

    // Resolve remote quota pools from live /usage PTY probe or cached snapshot
    let mut quota = QuotaInfo::default();
    if !cfg.general.fetch_tui_usage {
        quota.unavailability_reason = "Disabled by config (`fetch_tui_usage = false`).".to_string();
    }

    match usage_probe {
        Some(up) => {
            if up.connectivity == ConnectivityState::Online && !up.pools.is_empty() {
                sources.push("cli:agy /usage (PTY)".to_string());
                quota.remote_available = true;
                quota.pools = up.pools;
                quota.plan_tier = up.plan_tier;
                if up.email.is_some() {
                    local_meta.account.email = up.email;
                }
            } else {
                if let Some(auth_err) = up.auth_error {
                    local_meta.account.last_auth_error = Some(auth_err.clone());
                    local_meta.account.auth_state = classify_auth_error(&auth_err);
                }
                if let Some(ref err) = up.error {
                    if collection_error.is_none() {
                        collection_error = Some(err.clone());
                    }
                    quota.unavailability_reason = err.clone();
                }
                if let Some(prev) = cached {
                    if prev.quota.remote_available && !prev.quota.pools.is_empty() {
                        quota.remote_available = true;
                        quota.pools = prev.quota.pools.clone();
                        quota.plan_tier = prev.quota.plan_tier.clone();
                        stale = true;
                        if stale_reason.is_none() {
                            stale_reason = up.error;
                        }
                    }
                }
            }
        }
        None => {
            if let Some(prev) = cached {
                if prev.quota.remote_available && !prev.quota.pools.is_empty() {
                    quota.remote_available = true;
                    quota.pools = prev.quota.pools.clone();
                    quota.plan_tier = prev.quota.plan_tier.clone();
                    if prev.sources_checked.iter().any(|s| s.contains("/usage")) {
                        sources.push("cache:agy /usage".to_string());
                    }
                }
            }
        }
    }

    refresh_quota_countdowns(&mut quota, now);
    compute_quota_rollups(&mut quota, active_model_label.as_deref());

    if let Some(budget) = cfg.general.local_5h_token_budget {
        if budget > 0 {
            let used = if cfg.general.budget_include_cache_read {
                transcripts.window_5h.combined_total_tokens
            } else {
                transcripts.window_5h.fresh_total_tokens
            };
            let remaining = budget.saturating_sub(used);
            let used_pct = ((used as f64 / budget as f64) * 100.0).min(100.0);
            let remaining_pct = (100.0 - used_pct).max(0.0);
            quota.local_budget = Some(LocalBudgetStatus {
                budget_tokens: budget,
                used_tokens: used,
                remaining_tokens: remaining,
                used_pct,
                remaining_pct,
                include_cache_read: cfg.general.budget_include_cache_read,
            });
        }
    }

    // Merge live available models with historical per-model local token statistics and pool quotas
    let mut models = Vec::new();
    let mut seen_labels = HashSet::new();

    for entry in &live_entries {
        let stats = transcripts
            .model_totals
            .get(&entry.display_name)
            .cloned()
            .unwrap_or_default();
        seen_labels.insert(entry.display_name.clone());
        let is_active = active_model_label
            .as_deref()
            .map(|a| a.eq_ignore_ascii_case(&entry.display_name))
            .unwrap_or(false);
        models.push(build_model_info(
            entry.id.clone(),
            entry.display_name.clone(),
            true,
            is_active,
            stats,
            &quota.pools,
        ));
    }

    let mut historical_only: Vec<(String, ModelUsageTotals)> = transcripts
        .model_totals
        .iter()
        .filter(|(label, _)| !seen_labels.contains(*label))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    historical_only.sort_by_key(|a| std::cmp::Reverse(a.1.steps_all_time));
    for (label, stats) in historical_only {
        let is_active = active_model_label
            .as_deref()
            .map(|a| a.eq_ignore_ascii_case(&label))
            .unwrap_or(false);
        models.push(build_model_info(
            "historical-local".to_string(),
            label,
            false,
            is_active,
            stats,
            &quota.pools,
        ));
    }

    // Merge SQLite historical daily_buckets so heatmap keeps older days even if transcripts rotate
    let daily_buckets = merge_daily_with_db(&paths.history_db, &transcripts.daily_buckets);

    UsageSnapshot {
        collected_at: now,
        last_live_refresh_at,
        agy_version,
        connectivity,
        stale,
        stale_reason,
        collection_error,
        account: local_meta.account,
        active_model_label,
        quota,
        models,
        window_5h: transcripts.window_5h,
        window_today: transcripts.window_today,
        window_7d: transcripts.window_7d,
        window_30d: transcripts.window_30d,
        window_all_time: transcripts.window_all_time,
        daily_buckets,
        top_conversations: transcripts.top_conversations,
        active_statusline,
        sources_checked: sources,
    }
}

fn classify_auth_error(auth_err: &str) -> AuthState {
    let low = auth_err.to_lowercase();
    if low.contains("verify") {
        AuthState::VerificationRequired
    } else if low.contains("not logged") {
        AuthState::NotLoggedIn
    } else {
        AuthState::Expired
    }
}

pub fn refresh_quota_countdowns(quota: &mut QuotaInfo, now: DateTime<Utc>) {
    for pool in &mut quota.pools {
        update_window_countdown(pool.five_hour.as_mut(), now);
        update_window_countdown(pool.weekly.as_mut(), now);
    }
    if let Some(ra) = quota.reset_at {
        let rem = (ra - now).num_seconds().max(0);
        quota.reset_countdown_secs = Some(rem);
        quota.reset_in_text = Some(if rem == 0 {
            "Due now".to_string()
        } else {
            format_duration_secs(rem)
        });
    }
}

fn update_window_countdown(win: Option<&mut QuotaWindow>, now: DateTime<Utc>) {
    let Some(w) = win else {
        return;
    };
    if let Some(ra) = w.refreshes_at {
        let rem = (ra - now).num_seconds().max(0);
        w.refresh_in_secs = Some(rem);
        w.refresh_in_text = Some(if rem == 0 {
            "Due now".to_string()
        } else {
            format_duration_secs(rem)
        });
    }
}

pub fn compute_quota_rollups(quota: &mut QuotaInfo, active_model: Option<&str>) {
    if quota.pools.is_empty() {
        return;
    }

    let pool_limits: Vec<(&QuotaPool, &QuotaWindow)> = quota
        .pools
        .iter()
        .filter_map(|p| p.limiting_window().map(|w| (p, w)))
        .collect();

    if let Some((_, min_w)) = pool_limits
        .iter()
        .min_by(|a, b| a.1.remaining_pct.total_cmp(&b.1.remaining_pct))
    {
        quota.overall_lowest_pct = Some(min_w.remaining_pct);
    }
    if !pool_limits.is_empty() {
        let sum: f64 = pool_limits.iter().map(|(_, w)| w.remaining_pct).sum();
        quota.overall_avg_pct = Some(sum / pool_limits.len() as f64);
    }

    let active_pool = active_model
        .and_then(|m| quota.pools.iter().find(|p| p.matches_model(m)))
        .or_else(|| quota.pools.first());

    if let Some(pool) = active_pool {
        quota.active_pool_name = Some(pool.pool_name.clone());
        if let Some(win) = pool.limiting_window() {
            quota.active_pool_remaining_pct = Some(win.remaining_pct);
            quota.active_pool_reset_text = win.refresh_in_text.clone();
            quota.remaining_pct = Some(win.remaining_pct);
            quota.used_pct = Some(win.used_pct);
            quota.reset_at = win.refreshes_at;
            quota.reset_countdown_secs = win.refresh_in_secs;
            quota.reset_in_text = win.refresh_in_text.clone();
        }
    }
}

fn build_model_info(
    id: String,
    display_name: String,
    available: bool,
    is_active: bool,
    stats: ModelUsageTotals,
    pools: &[QuotaPool],
) -> ModelInfo {
    let matched_pool = pools
        .iter()
        .find(|p| p.matches_model(&display_name) || p.matches_model(&id));
    let five_hour_pct = matched_pool.and_then(|p| p.five_hour.as_ref().map(|w| w.remaining_pct));
    let weekly_pct = matched_pool.and_then(|p| p.weekly.as_ref().map(|w| w.remaining_pct));
    let lim = matched_pool.and_then(|p| p.limiting_window());

    ModelInfo {
        id,
        family: model_family_from_label(&display_name),
        display_name,
        available,
        is_active,
        steps_all_time: stats.steps_all_time,
        steps_5h: stats.steps_5h,
        input_tokens: stats.input_tokens,
        cache_read_tokens: stats.cache_read_tokens,
        output_tokens: stats.output_tokens,
        thinking_tokens: stats.thinking_tokens,
        response_tokens: stats.response_tokens,
        remote_quota_pct: lim.map(|w| w.remaining_pct),
        five_hour_pct,
        weekly_pct,
        reset_in_text: lim.and_then(|w| w.refresh_in_text.clone()),
        pool_name: matched_pool.map(|p| p.pool_name.clone()),
    }
}

fn merge_daily_with_db(
    db_path: &std::path::Path,
    live_buckets: &[DailyTokenBucket],
) -> Vec<DailyTokenBucket> {
    let mut map: BTreeMap<String, DailyTokenBucket> = BTreeMap::new();
    if let Ok(db) = HistoryDatabase::open(db_path) {
        if let Ok(rows) = db.query_daily_summaries(400) {
            for r in rows {
                map.insert(r.day_local.clone(), r);
            }
        }
    }
    for b in live_buckets {
        map.entry(b.day_local.clone())
            .and_modify(|existing| {
                existing.planner_steps = existing.planner_steps.max(b.planner_steps);
                existing.input_tokens = existing.input_tokens.max(b.input_tokens);
                existing.cache_read_tokens = existing.cache_read_tokens.max(b.cache_read_tokens);
                existing.output_tokens = existing.output_tokens.max(b.output_tokens);
                existing.thinking_tokens = existing.thinking_tokens.max(b.thinking_tokens);
                existing.response_tokens = existing.response_tokens.max(b.response_tokens);
                existing.conversations_active =
                    existing.conversations_active.max(b.conversations_active);
            })
            .or_insert_with(|| b.clone());
    }
    map.into_values().collect()
}
