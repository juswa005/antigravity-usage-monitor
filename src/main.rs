use agmonitor::cache::{
    is_live_cache_fresh, load_snapshot_cache, save_active_statusline, try_acquire_refresh_lock,
};
use agmonitor::cli::{Cli, CliHeatmapMetric, Commands, HistoryArgs};
use agmonitor::collector::{collect_usage, CollectOptions};
use agmonitor::config::{load_config, AppConfig, AppPaths};
use agmonitor::database::HistoryDatabase;
use agmonitor::doctor::run_doctor;
use agmonitor::models::{ActiveStatusLine, CheckSeverity};
use agmonitor::tui::heatmap::{render_ansi_heatmap, HeatmapGrid, HeatmapMetric};
use agmonitor::tui::run_tui;
use agmonitor::utils::{
    display_email, format_age, format_duration_secs, format_local_datetime, format_tokens,
    format_tokens_exact, shorten_path_home,
};
use agmonitor::waybar::build_waybar_payload;
use chrono::{Local, Utc};
use clap::Parser;
use serde_json::Value;
use std::io::{self, Read};
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let (mut cfg, paths) = load_config(cli.config.as_deref());
    if cli.unmask {
        cfg.general.mask_email = false;
    }

    match cli.command.unwrap_or(Commands::Tui) {
        Commands::Tui => {
            if let Err(e) = run_tui(cfg, paths) {
                eprintln!("Error running TUI: {e}");
                return ExitCode::from(1);
            }
            ExitCode::SUCCESS
        }
        Commands::Status(args) => cmd_status(&cfg, &paths, args.json),
        Commands::Refresh(args) => cmd_refresh(&cfg, &paths, args.json, args.signal_waybar),
        Commands::Models(args) => cmd_models(&cfg, &paths, args.json),
        Commands::History(args) => cmd_history(&cfg, &paths, args),
        Commands::Tokens(args) => cmd_tokens(&cfg, &paths, args.json),
        Commands::Waybar(args) => cmd_waybar(&cfg, &paths, args.refresh, args.non_blocking),
        Commands::Doctor(args) => cmd_doctor(&cfg, &paths, args.json),
        Commands::StatuslineHook => cmd_statusline_hook(&paths),
    }
}

fn cmd_status(cfg: &AppConfig, paths: &AppPaths, json: bool) -> ExitCode {
    let snap = collect_usage(cfg, paths, CollectOptions::default());
    if json {
        return print_json(&snap);
    }

    let now = Utc::now();
    let acct = display_email(snap.account.email.as_deref(), cfg.general.mask_email);
    let tier = snap
        .quota
        .plan_tier
        .as_deref()
        .map(|t| format!(" · {t}"))
        .unwrap_or_default();
    let avail_cnt = snap.models.iter().filter(|m| m.available).count();

    println!("Antigravity Usage Monitor");
    println!(
        "  Account:      {acct}{tier} ({})",
        snap.account.auth_state.as_label()
    );
    println!(
        "  Connectivity: {}{}",
        snap.connectivity.as_label(),
        if snap.stale { " [stale]" } else { "" }
    );
    println!(
        "  Active Model: {}",
        snap.active_model_label.as_deref().unwrap_or("Unavailable")
    );
    println!("  Live Models:  {} available", avail_cnt);

    if snap.quota.remote_available && !snap.quota.pools.is_empty() {
        println!();
        println!("Remaining Quota (/usage):");
        for p in &snap.quota.pools {
            let mut parts = Vec::new();
            if let Some(ref fh) = p.five_hour {
                let rst = fh.refresh_in_text.as_deref().unwrap_or("—");
                parts.push(format!("5h: {:>6.2}% ({rst})", fh.remaining_pct));
            }
            if let Some(ref wk) = p.weekly {
                let rst = wk.refresh_in_text.as_deref().unwrap_or("—");
                parts.push(format!("Weekly: {:>6.2}% ({rst})", wk.remaining_pct));
            }
            println!("  {:<24} {}", p.pool_name, parts.join("  |  "));
        }
    } else {
        println!(
            "  Remote Quota: Unavailable ({})",
            snap.quota.unavailability_reason
        );
    }

    if let Some(ref b) = snap.quota.local_budget {
        println!(
            "  Local 5h Budget: {:.1}% remaining ({} / {} tokens)",
            b.remaining_pct,
            format_tokens_exact(b.used_tokens),
            format_tokens_exact(b.budget_tokens)
        );
    }

    println!();
    println!("Actual Local Token Consumption (Transcripts):");
    for w in [
        &snap.window_5h,
        &snap.window_today,
        &snap.window_7d,
        &snap.window_all_time,
    ] {
        println!(
            "  {:<15} Fresh: {:>8}  (In: {:>8}, Out: {:>7}, Cache: {:>8}, Steps: {})",
            w.window_name,
            format_tokens(w.fresh_total_tokens),
            format_tokens(w.input_tokens),
            format_tokens(w.output_tokens),
            format_tokens(w.cache_read_tokens),
            format_tokens_exact(w.planner_steps)
        );
    }

    if let Some(live_at) = snap.last_live_refresh_at {
        println!(
            "\nLast live refresh: {} ({})",
            format_local_datetime(live_at),
            format_age(live_at, now)
        );
    }
    if let Some(ref err) = snap.collection_error {
        println!("Note/Error: {err}");
    }
    ExitCode::SUCCESS
}

fn cmd_refresh(cfg: &AppConfig, paths: &AppPaths, json: bool, signal_waybar: bool) -> ExitCode {
    let snap = collect_usage(
        cfg,
        paths,
        CollectOptions {
            force_refresh: true,
            cache_only: false,
        },
    );

    if signal_waybar {
        signal_waybar_module(cfg.waybar.signal);
    }

    if json {
        return print_json(&snap);
    }

    let avail_cnt = snap.models.iter().filter(|m| m.available).count();
    let quota_summary = match (
        snap.quota.active_pool_remaining_pct,
        snap.quota.active_pool_reset_text.as_deref(),
    ) {
        (Some(pct), Some(rst)) => format!("{pct:.2}% left ({rst})"),
        (Some(pct), None) => format!("{pct:.2}% left"),
        _ => "Unavailable".to_string(),
    };
    println!(
        "Refreshed at {} | {} | Auth: {} | Models: {} | Active Quota: {} | 5h fresh: {}",
        format_local_datetime(snap.collected_at),
        snap.connectivity.as_label(),
        snap.account.auth_state.as_label(),
        avail_cnt,
        quota_summary,
        format_tokens_exact(snap.window_5h.fresh_total_tokens),
    );
    if let Some(ref err) = snap.collection_error {
        eprintln!("Warning: {err}");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

fn cmd_models(cfg: &AppConfig, paths: &AppPaths, json: bool) -> ExitCode {
    let snap = collect_usage(cfg, paths, CollectOptions::default());
    if json {
        return print_json(&snap.models);
    }

    println!(
        "{:<29} {:<10} {:>7} {:>7} {:<10} {:>11} {:>8} {:>9} {:>8}",
        "Model", "Status", "5h", "Weekly", "Reset", "5h/All Stp", "Input", "Cache", "Output"
    );
    println!("{}", "-".repeat(107));
    for m in &snap.models {
        let marker = if m.is_active { "* " } else { "  " };
        let name = format!("{marker}{}", m.display_name);
        let status = if m.available {
            "Available"
        } else {
            "Historical"
        };
        let fh = m
            .five_hour_pct
            .map(|q| format!("{q:.1}%"))
            .unwrap_or_else(|| "—".to_string());
        let wk = m
            .weekly_pct
            .map(|q| format!("{q:.1}%"))
            .unwrap_or_else(|| "—".to_string());
        let rst = m
            .reset_in_text
            .as_deref()
            .map(|s| if s == "Quota available" { "Ready" } else { s })
            .unwrap_or("—");
        println!(
            "{:<29} {:<10} {:>7} {:>7} {:<10} {:>11} {:>8} {:>9} {:>8}",
            truncate(&name, 29),
            status,
            fh,
            wk,
            truncate(rst, 10),
            format!("{}/{}", m.steps_5h, m.steps_all_time),
            format_tokens(m.input_tokens),
            format_tokens(m.cache_read_tokens),
            format_tokens(m.output_tokens),
        );
    }
    println!("\n* = active model in settings.json | Quota columns sourced from `agy /usage`.");
    ExitCode::SUCCESS
}

fn cmd_history(cfg: &AppConfig, paths: &AppPaths, args: HistoryArgs) -> ExitCode {
    let _ = collect_usage(
        cfg,
        paths,
        CollectOptions {
            force_refresh: false,
            cache_only: true,
        },
    );
    let Ok(db) = HistoryDatabase::open(&paths.history_db) else {
        eprintln!(
            "Failed to open history database at {}",
            paths.history_db.display()
        );
        return ExitCode::from(1);
    };

    let days_limit = args.days.unwrap_or(cfg.history.default_days);
    let all_daily = db.query_daily_summaries(400).unwrap_or_default();
    let daily: Vec<_> = all_daily
        .iter()
        .rev()
        .take(days_limit)
        .cloned()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let weekly = db.query_weekly_summaries(12).unwrap_or_default();
    let snapshots = db.query_recent_snapshots(10).unwrap_or_default();

    let metric = match args.metric {
        CliHeatmapMetric::Fresh => HeatmapMetric::FreshTokens,
        CliHeatmapMetric::Combined => HeatmapMetric::CombinedTokens,
        CliHeatmapMetric::Steps => HeatmapMetric::PlannerSteps,
    };

    if args.json {
        let grid = HeatmapGrid::build(&all_daily, Local::now().date_naive(), args.weeks, metric);
        let heatmap_cells: Vec<Value> = grid
            .cells_flat
            .iter()
            .map(|c| {
                serde_json::json!({
                    "date": c.date.format("%Y-%m-%d").to_string(),
                    "value": c.value,
                    "level": c.level,
                })
            })
            .collect();
        let payload = serde_json::json!({
            "heatmap": {
                "metric": metric.label(),
                "weeks": args.weeks,
                "active_days": grid.active_days,
                "current_streak": grid.current_streak,
                "longest_streak": grid.longest_streak,
                "cells": heatmap_cells,
            },
            "daily": daily,
            "weekly": weekly,
            "recent_snapshots": snapshots,
        });
        return print_json(&payload);
    }

    println!(
        "{}",
        render_ansi_heatmap(&all_daily, args.weeks.clamp(4, 52), metric)
    );

    println!("DAILY TOKEN USAGE (last {days_limit} recorded days)");
    println!(
        "{:<12} {:>7} {:>11} {:>12} {:>10} {:>12} {:>6}",
        "Date", "Steps", "Input", "Cache Read", "Output", "Fresh Total", "Convs"
    );
    println!("{}", "-".repeat(76));
    for d in daily.iter().rev() {
        println!(
            "{:<12} {:>7} {:>11} {:>12} {:>10} {:>12} {:>6}",
            d.day_local,
            d.planner_steps,
            format_tokens_exact(d.input_tokens),
            format_tokens_exact(d.cache_read_tokens),
            format_tokens_exact(d.output_tokens),
            format_tokens_exact(d.input_tokens + d.output_tokens),
            d.conversations_active
        );
    }

    println!("\nWEEKLY SUMMARIES");
    println!(
        "{:<10} {:<23} {:>5} {:>7} {:>12} {:>12}",
        "Week", "Span", "Days", "Steps", "Fresh Total", "Cache Read"
    );
    println!("{}", "-".repeat(74));
    for w in weekly.iter().rev() {
        println!(
            "{:<10} {:<23} {:>5} {:>7} {:>12} {:>12}",
            w.iso_week,
            format!("{} .. {}", w.start_day, w.end_day),
            w.active_days,
            w.planner_steps,
            format_tokens_exact(w.input_tokens + w.output_tokens),
            format_tokens_exact(w.cache_read_tokens),
        );
    }

    println!("\nRECENT COLLECTED SNAPSHOTS ({} shown)", snapshots.len());
    for s in &snapshots {
        let q = s
            .remote_quota_pct
            .map(|p| format!("{p:.1}%"))
            .unwrap_or_else(|| "N/A".to_string());
        println!(
            "  #{}  {}  [{} / {}]  quota={}  model={}  5h_fresh={}",
            s.id,
            s.collected_at,
            s.auth_state,
            s.connectivity,
            q,
            s.active_model.as_deref().unwrap_or("N/A"),
            format_tokens(s.tokens_5h_fresh),
        );
    }
    ExitCode::SUCCESS
}

fn cmd_tokens(cfg: &AppConfig, paths: &AppPaths, json: bool) -> ExitCode {
    let snap = collect_usage(
        cfg,
        paths,
        CollectOptions {
            force_refresh: false,
            cache_only: true,
        },
    );
    if json {
        let payload = serde_json::json!({
            "actual_windows": {
                "5h": snap.window_5h,
                "today": snap.window_today,
                "7d": snap.window_7d,
                "30d": snap.window_30d,
                "all_time": snap.window_all_time,
            },
            "remaining_token_allowance": snap.quota.remaining_token_allowance,
            "remaining_token_allowance_status": "unavailable",
            "top_conversations": snap.top_conversations,
        });
        return print_json(&payload);
    }

    println!("ACTUAL TOKEN STATISTICS (Verified from Antigravity Transcripts)");
    println!(
        "Remaining Token Allowance: Unavailable (Antigravity tracks % compute quotas via `/usage`)"
    );
    println!();
    for w in [
        &snap.window_5h,
        &snap.window_today,
        &snap.window_7d,
        &snap.window_30d,
        &snap.window_all_time,
    ] {
        println!(
            "  {:<15} In: {:>11} | Cache: {:>12} | Out: {:>9} (Think: {}, Text: {}) | Fresh: {:>11} | Steps: {}",
            w.window_name,
            format_tokens_exact(w.input_tokens),
            format_tokens_exact(w.cache_read_tokens),
            format_tokens_exact(w.output_tokens),
            format_tokens(w.thinking_tokens),
            format_tokens(w.response_tokens),
            format_tokens_exact(w.fresh_total_tokens),
            format_tokens_exact(w.planner_steps),
        );
    }

    println!("\nTOP CONVERSATIONS (by fresh Input+Output tokens)");
    for c in snap.top_conversations.iter().take(10) {
        println!(
            "  {:<34} {:<24} {:>5} steps  {:>9} fresh  {:>9} cache  {}",
            truncate(&c.title, 34),
            truncate(c.primary_model.as_deref().unwrap_or("Unknown"), 24),
            c.planner_steps,
            format_tokens(c.input_tokens + c.output_tokens),
            format_tokens(c.cache_read_tokens),
            shorten_path_home(&c.workspace),
        );
    }
    ExitCode::SUCCESS
}

fn cmd_waybar(cfg: &AppConfig, paths: &AppPaths, refresh: bool, non_blocking: bool) -> ExitCode {
    let now = Utc::now();
    let snap = if refresh {
        let s = collect_usage(
            cfg,
            paths,
            CollectOptions {
                force_refresh: true,
                cache_only: false,
            },
        );
        signal_waybar_module(cfg.waybar.signal);
        s
    } else if non_blocking {
        let cached = load_snapshot_cache(&paths.snapshot_cache);
        let need_bg = cached
            .as_ref()
            .map(|c| {
                !is_live_cache_fresh(c, cfg.general.refresh_interval_secs, now)
                    || (cfg.general.fetch_tui_usage && !c.quota.remote_available)
            })
            .unwrap_or(true);
        if need_bg {
            spawn_detached_refresh(cfg, paths);
        }
        collect_usage(
            cfg,
            paths,
            CollectOptions {
                force_refresh: false,
                cache_only: true,
            },
        )
    } else {
        collect_usage(cfg, paths, CollectOptions::default())
    };

    let payload = build_waybar_payload(&snap, cfg);
    // Waybar custom modules parse one JSON object per line
    match serde_json::to_string(&payload) {
        Ok(s) => {
            println!("{s}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("JSON serialization error: {e}");
            ExitCode::from(1)
        }
    }
}

fn cmd_doctor(cfg: &AppConfig, paths: &AppPaths, json: bool) -> ExitCode {
    let report = run_doctor(cfg, paths);
    if json {
        let _ = print_json(&report);
        return ExitCode::from(report.overall_exit_code as u8);
    }

    println!("Antigravity Usage Monitor — Doctor Diagnostics");
    println!("{}", "-".repeat(60));
    for chk in &report.checks {
        let tag = match chk.status {
            CheckSeverity::Ok => "[OK]  ",
            CheckSeverity::Warn => "[WARN]",
            CheckSeverity::Error => "[ERR] ",
        };
        println!("{tag} {}: {}", chk.name, chk.detail);
    }
    println!("\nConfig path: {}", paths.config_file.display());
    println!("Cache path:  {}", paths.snapshot_cache.display());
    println!("DB path:     {}", paths.history_db.display());
    println!(
        "Refresh TTL: {} (CLI timeout {}s, /usage timeout {}s)",
        format_duration_secs(cfg.general.refresh_interval_secs as i64),
        cfg.general.cli_timeout_secs,
        cfg.general.usage_timeout_secs
    );
    ExitCode::from(report.overall_exit_code as u8)
}

fn cmd_statusline_hook(paths: &AppPaths) -> ExitCode {
    let mut buf = String::new();
    if io::stdin().read_to_string(&mut buf).is_err() || buf.trim().is_empty() {
        return ExitCode::SUCCESS;
    }
    let Ok(val) = serde_json::from_str::<Value>(&buf) else {
        return ExitCode::SUCCESS;
    };
    let item = ActiveStatusLine {
        recorded_at: Utc::now(),
        agent_state: val
            .get("agent_state")
            .and_then(|v| v.as_str())
            .map(ToString::to_string),
        model_display_name: val
            .get("model")
            .and_then(|m| m.get("display_name"))
            .and_then(|v| v.as_str())
            .map(ToString::to_string),
        context_used_pct: val
            .get("context_window")
            .and_then(|c| c.get("used_percentage"))
            .and_then(|v| v.as_f64()),
        conversation_title: val
            .get("conversation_title")
            .and_then(|v| v.as_str())
            .map(ToString::to_string),
    };
    let _ = save_active_statusline(&paths.statusline_cache, &item);
    ExitCode::SUCCESS
}

fn spawn_detached_refresh(cfg: &AppConfig, paths: &AppPaths) {
    let lock_ttl =
        Duration::from_secs(cfg.general.cli_timeout_secs + cfg.general.usage_timeout_secs + 15);
    let Some(guard) = try_acquire_refresh_lock(&paths.refresh_lock, lock_ttl) else {
        return;
    };
    drop(guard);

    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let mut cmd = Command::new(exe);
    if paths.config_file.exists() {
        cmd.arg("--config").arg(&paths.config_file);
    }
    cmd.arg("refresh")
        .arg("--signal-waybar")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let _ = cmd.spawn();
}

fn signal_waybar_module(signal: u32) {
    if signal == 0 {
        return;
    }
    let _ = Command::new("pkill")
        .arg(format!("-RTMIN+{signal}"))
        .arg("waybar")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn print_json<T: serde::Serialize>(value: &T) -> ExitCode {
    match serde_json::to_string_pretty(value) {
        Ok(s) => {
            println!("{s}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("JSON serialization error: {e}");
            ExitCode::from(1)
        }
    }
}

fn truncate(s: &str, max_chars: usize) -> String {
    let count = s.chars().count();
    if count <= max_chars {
        s.to_string()
    } else if max_chars > 1 {
        let mut out: String = s.chars().take(max_chars - 1).collect();
        out.push('…');
        out
    } else {
        "…".to_string()
    }
}
