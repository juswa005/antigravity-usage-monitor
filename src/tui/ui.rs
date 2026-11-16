use crate::config::{AppConfig, AppPaths};
use crate::models::{
    AuthState, CheckSeverity, ConnectivityState, DailyTokenBucket, DoctorReport,
    SnapshotHistoryRow, UsageSnapshot, WeeklySummaryRow,
};
use crate::tui::heatmap::{render_ratatui_heatmap, HeatmapMetric};
use crate::tui::theme;
use crate::utils::{
    display_email, format_age, format_duration_secs, format_local_time, format_tokens,
    format_tokens_exact, shorten_path_home,
};
use chrono::Utc;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Bar, BarChart, BarGroup, Block, Borders, Cell, Clear, Gauge, Paragraph, Row, Table, TableState,
    Tabs, Wrap,
};
use ratatui::Frame;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveTab {
    Dashboard,
    Models,
    History,
    Tokens,
    Doctor,
}

impl ActiveTab {
    pub const ALL: [ActiveTab; 5] = [
        ActiveTab::Dashboard,
        ActiveTab::Models,
        ActiveTab::History,
        ActiveTab::Tokens,
        ActiveTab::Doctor,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Dashboard => "1 Dashboard",
            Self::Models => "2 Models",
            Self::History => "3 History",
            Self::Tokens => "4 Tokens",
            Self::Doctor => "5 Doctor",
        }
    }

    pub fn index(self) -> usize {
        match self {
            Self::Dashboard => 0,
            Self::Models => 1,
            Self::History => 2,
            Self::Tokens => 3,
            Self::Doctor => 4,
        }
    }
}

pub struct TuiAppState<'a> {
    pub tab: ActiveTab,
    pub snapshot: &'a UsageSnapshot,
    pub daily_history: &'a [DailyTokenBucket],
    pub weekly_history: &'a [WeeklySummaryRow],
    pub snapshot_history: &'a [SnapshotHistoryRow],
    pub doctor_report: Option<&'a DoctorReport>,
    pub cfg: &'a AppConfig,
    pub paths: &'a AppPaths,
    pub table_scroll: usize,
    pub heatmap_metric: HeatmapMetric,
    pub heatmap_back_offset: usize,
    pub refreshing: bool,
    pub status_banner: Option<&'a str>,
    pub show_help: bool,
}

pub fn render(frame: &mut Frame, app: &TuiAppState<'_>) {
    let area = frame.area();
    frame.render_widget(Block::default().style(theme::base()), area);

    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Length(2), // Tabs
            Constraint::Min(8),    // Body
            Constraint::Length(2), // Footer
        ])
        .split(area);

    render_header(frame, root[0], app);
    render_tabs(frame, root[1], app);

    match app.tab {
        ActiveTab::Dashboard => render_dashboard(frame, root[2], app),
        ActiveTab::Models => render_models_tab(frame, root[2], app),
        ActiveTab::History => render_history_tab(frame, root[2], app),
        ActiveTab::Tokens => render_tokens_tab(frame, root[2], app),
        ActiveTab::Doctor => render_doctor_tab(frame, root[2], app),
    }

    render_footer(frame, root[3], app);

    if app.show_help {
        render_help_overlay(frame, area);
    }
}

fn render_header(frame: &mut Frame, area: Rect, app: &TuiAppState<'_>) {
    let now = Utc::now();
    let snap = app.snapshot;
    let acct = display_email(snap.account.email.as_deref(), app.cfg.general.mask_email);

    let auth_style = match snap.account.auth_state {
        AuthState::Authenticated => theme::ok_style(),
        AuthState::Unknown => theme::warn_style(),
        AuthState::Expired | AuthState::NotLoggedIn | AuthState::VerificationRequired => {
            theme::crit_style()
        }
    };

    let conn_style = match snap.connectivity {
        ConnectivityState::Online => theme::ok_style(),
        ConnectivityState::CachedOnly => theme::warn_style(),
        ConnectivityState::Offline | ConnectivityState::Error | ConnectivityState::CliMissing => {
            theme::crit_style()
        }
    };

    let updated = snap
        .last_live_refresh_at
        .map(|t| format!("{} ({})", format_local_time(t), format_age(t, now)))
        .unwrap_or_else(|| "Local only".to_string());

    let ver = snap
        .agy_version
        .as_deref()
        .map(|v| format!(" v{v}"))
        .unwrap_or_default();

    let mut spans = vec![
        Span::styled(" ANTIGRAVITY USAGE MONITOR ", theme::title()),
        Span::styled(format!("(agy{ver}) "), theme::muted()),
        Span::raw("│ "),
        Span::styled(
            acct,
            Style::default().fg(theme::FG).add_modifier(Modifier::BOLD),
        ),
    ];

    if let Some(ref tier) = snap.quota.plan_tier {
        spans.push(Span::styled(
            format!(" ({tier})"),
            Style::default().fg(theme::BRIGHT_TEAL),
        ));
    }

    spans.extend([
        Span::raw(" ["),
        Span::styled(snap.account.auth_state.as_label(), auth_style),
        Span::raw("] │ "),
        Span::styled(snap.connectivity.as_label(), conn_style),
    ]);

    if snap.stale {
        spans.push(Span::styled(" [STALE]", theme::warn_style()));
    }
    if app.refreshing {
        spans.push(Span::styled(
            " [REFRESHING...]",
            Style::default().fg(theme::BRIGHT_TEAL),
        ));
    }
    spans.push(Span::raw(" │ Live: "));
    spans.push(Span::styled(updated, theme::muted()));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::border())
        .style(theme::surface());
    let p = Paragraph::new(Line::from(spans)).block(block);
    frame.render_widget(p, area);
}

fn render_tabs(frame: &mut Frame, area: Rect, app: &TuiAppState<'_>) {
    let titles: Vec<Line> = ActiveTab::ALL
        .iter()
        .map(|t| Line::from(Span::raw(format!(" {} ", t.label()))))
        .collect();

    let tabs = Tabs::new(titles)
        .select(app.tab.index())
        .style(theme::muted())
        .highlight_style(
            Style::default()
                .fg(theme::BG)
                .bg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        )
        .divider(Span::styled("│", theme::border()));

    frame.render_widget(tabs, area);
}

fn render_dashboard(frame: &mut Frame, area: Rect, app: &TuiAppState<'_>) {
    let wide = area.width >= 96;
    let top_height = if wide { 17 } else { 26 };
    let v_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(top_height), Constraint::Min(6)])
        .split(area);

    let top_chunks = if wide {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(46), Constraint::Percentage(54)])
            .split(v_chunks[0])
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(v_chunks[0])
    };

    render_quota_card(frame, top_chunks[0], app);

    let right_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(10), Constraint::Min(6)])
        .split(top_chunks[1]);

    let hm_title = format!(" ACTIVITY HEATMAP — [G] {} ", app.heatmap_metric.label());
    render_ratatui_heatmap(
        frame,
        right_chunks[0],
        app.daily_history,
        app.heatmap_metric,
        None,
        &hm_title,
        true,
    );
    render_token_windows_card(frame, right_chunks[1], app);

    render_model_table(frame, v_chunks[1], app, true);
}

fn render_quota_card(frame: &mut Frame, area: Rect, app: &TuiAppState<'_>) {
    let snap = app.snapshot;
    let now = Utc::now();

    let block = Block::default()
        .title(Span::styled(
            " QUOTA & POOL LIMITS (/usage) ",
            theme::title(),
        ))
        .borders(Borders::ALL)
        .border_style(theme::border())
        .style(theme::base());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let active_model = snap.active_model_label.as_deref().unwrap_or("Unavailable");

    let expiry_text = match snap.account.token_expiry {
        Some(exp) => {
            let diff = (exp - now).num_seconds();
            if diff > 0 {
                format!(
                    "{} ({})",
                    format_local_time(exp),
                    format_duration_secs(diff)
                )
            } else {
                format!("{} (auto-refresh)", format_local_time(exp))
            }
        }
        None => "Unavailable".to_string(),
    };

    if snap.quota.remote_available && !snap.quota.pools.is_empty() {
        let mut lines: Vec<Line> = Vec::new();
        let active_pct_str = snap
            .quota
            .active_pool_remaining_pct
            .map(|p| format!("{p:.2}% left"))
            .unwrap_or_else(|| "N/A".to_string());
        let active_reset_str = snap.quota.active_pool_reset_text.as_deref().unwrap_or("—");

        lines.push(Line::from(vec![
            Span::styled("Active: ", theme::muted()),
            Span::styled(
                active_model,
                Style::default()
                    .fg(theme::BRIGHT_TEAL)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  [{active_pct_str} · {active_reset_str}]"),
                theme::ok_style(),
            ),
        ]));

        if let (Some(low), Some(avg)) = (snap.quota.overall_lowest_pct, snap.quota.overall_avg_pct)
        {
            lines.push(Line::from(vec![
                Span::styled("Pools Summary: ", theme::muted()),
                Span::styled(
                    format!("Lowest {low:.1}%  │  Mean {avg:.1}%"),
                    Style::default().fg(theme::FG),
                ),
                Span::styled(format!("  │ OAuth: {expiry_text}"), theme::muted()),
            ]));
        }

        lines.push(Line::from(""));

        for p in &snap.quota.pools {
            let short_name = if p.pool_name.contains("CLAUDE") {
                "Claude & GPT"
            } else if p.pool_name.contains("ARGON") {
                "Gemini 4 Argon"
            } else {
                "Gemini (Hyd/Flash)"
            };
            let is_active_pool = snap
                .quota
                .active_pool_name
                .as_deref()
                .map(|n| n == p.pool_name)
                .unwrap_or(false);
            let prefix = if is_active_pool { "★ " } else { "  " };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{prefix}{short_name}"),
                    Style::default()
                        .fg(if is_active_pool {
                            theme::BRIGHT_TEAL
                        } else {
                            theme::FG
                        })
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    if p.models_included.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", p.models_included.join(", "))
                    },
                    theme::muted(),
                ),
            ]));

            if let Some(ref fh) = p.five_hour {
                lines.push(render_pool_bar_line(
                    "  5h ",
                    fh.remaining_pct,
                    fh.refresh_in_text.as_deref(),
                    app.cfg,
                ));
            }
            if let Some(ref wk) = p.weekly {
                lines.push(render_pool_bar_line(
                    "  Wk ",
                    wk.remaining_pct,
                    wk.refresh_in_text.as_deref(),
                    app.cfg,
                ));
            }
        }

        if let Some(ref err) = snap.collection_error {
            lines.push(Line::from(vec![
                Span::styled("Note: ", theme::warn_style()),
                Span::styled(err.as_str(), theme::warn_style()),
            ]));
        }

        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
    } else if let Some(ref b) = snap.quota.local_budget {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(3)])
            .split(inner);

        let gauge_color = if b.remaining_pct < app.cfg.waybar.critical_remaining_pct {
            theme::CRITICAL
        } else if b.remaining_pct < app.cfg.waybar.warning_remaining_pct {
            theme::WARNING
        } else {
            theme::ACCENT
        };
        let gauge = Gauge::default()
            .block(
                Block::default()
                    .title(Span::styled(
                        " Local 5h Token Budget (Configured) ",
                        theme::muted(),
                    ))
                    .borders(Borders::NONE),
            )
            .gauge_style(Style::default().fg(gauge_color).bg(theme::SURFACE))
            .percent(b.remaining_pct.round().clamp(0.0, 100.0) as u16)
            .label(format!(
                "{:.1}% left ({} / {})",
                b.remaining_pct,
                format_tokens(b.used_tokens),
                format_tokens(b.budget_tokens)
            ));
        frame.render_widget(gauge, chunks[0]);

        let info = Paragraph::new(vec![
            Line::from(vec![
                Span::styled("Remote Quota: ", theme::muted()),
                Span::styled(&snap.quota.unavailability_reason, theme::warn_style()),
            ]),
            Line::from(vec![
                Span::styled("Active Model: ", theme::muted()),
                Span::styled(
                    active_model,
                    Style::default().fg(theme::FG).add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled("OAuth Expiry: ", theme::muted()),
                Span::raw(expiry_text),
            ]),
        ])
        .wrap(Wrap { trim: true });
        frame.render_widget(info, chunks[1]);
    } else {
        let avail_cnt = snap.models.iter().filter(|m| m.available).count();
        let mut lines = vec![
            Line::from(vec![
                Span::styled("Remote Quota:  ", theme::muted()),
                Span::styled("Unavailable", theme::warn_style()),
                Span::styled("  (press [R] to fetch `/usage`)", theme::muted()),
            ]),
            Line::from(vec![
                Span::styled("Active Model:  ", theme::muted()),
                Span::styled(
                    active_model,
                    Style::default()
                        .fg(theme::BRIGHT_TEAL)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled("Live Catalog:  ", theme::muted()),
                Span::raw(format!("{avail_cnt} available models")),
            ]),
            Line::from(vec![
                Span::styled("OAuth Expiry:  ", theme::muted()),
                Span::raw(expiry_text),
            ]),
            Line::from(vec![
                Span::styled("Rem. Tokens:   ", theme::muted()),
                Span::styled("Unavailable", theme::muted()),
                Span::styled("  (compute-quota model)", theme::muted()),
            ]),
        ];

        if let Some(ref err) = snap.collection_error {
            lines.push(Line::from(vec![
                Span::styled("Last Error:    ", theme::crit_style()),
                Span::styled(err.as_str(), theme::crit_style()),
            ]));
        }

        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
    }
}

fn render_pool_bar_line<'a>(
    label: &'a str,
    remaining_pct: f64,
    reset_text: Option<&'a str>,
    cfg: &AppConfig,
) -> Line<'a> {
    let total_blocks = 18usize;
    let filled = ((remaining_pct / 100.0) * total_blocks as f64)
        .round()
        .clamp(0.0, total_blocks as f64) as usize;
    let empty = total_blocks - filled;
    let bar_str = format!("{}{}", "█".repeat(filled), "░".repeat(empty));

    let bar_style = if remaining_pct < cfg.waybar.critical_remaining_pct {
        theme::crit_style()
    } else if remaining_pct < cfg.waybar.warning_remaining_pct {
        theme::warn_style()
    } else {
        Style::default().fg(theme::GREEN)
    };

    let reset_fmt = match reset_text {
        Some("Quota available") => "Available".to_string(),
        Some(s) => format!("Reset {s}"),
        None => String::new(),
    };

    Line::from(vec![
        Span::styled(label, theme::muted()),
        Span::styled(bar_str, bar_style),
        Span::styled(
            format!(" {:>6.2}%  ", remaining_pct),
            Style::default().fg(theme::FG).add_modifier(Modifier::BOLD),
        ),
        Span::styled(reset_fmt, theme::muted()),
    ])
}

fn render_token_windows_card(frame: &mut Frame, area: Rect, app: &TuiAppState<'_>) {
    let snap = app.snapshot;
    let block = Block::default()
        .title(Span::styled(
            " ACTUAL TOKEN CONSUMPTION (LOCAL TRANSCRIPTS) ",
            theme::title(),
        ))
        .borders(Borders::ALL)
        .border_style(theme::border())
        .style(theme::base());

    let header = Row::new([
        Cell::from("Window"),
        Cell::from("Fresh (In+Out)"),
        Cell::from("Input"),
        Cell::from("Cache Read"),
        Cell::from("Output"),
        Cell::from("Steps"),
    ])
    .style(
        Style::default()
            .fg(theme::BRIGHT_TEAL)
            .add_modifier(Modifier::BOLD),
    );

    let windows = [
        &snap.window_5h,
        &snap.window_today,
        &snap.window_7d,
        &snap.window_all_time,
    ];

    let rows: Vec<Row> = windows
        .iter()
        .map(|w| {
            Row::new([
                Cell::from(w.window_name.clone()),
                Cell::from(format_tokens(w.fresh_total_tokens))
                    .style(Style::default().fg(theme::FG).add_modifier(Modifier::BOLD)),
                Cell::from(format_tokens(w.input_tokens)),
                Cell::from(format_tokens(w.cache_read_tokens)).style(theme::muted()),
                Cell::from(format_tokens(w.output_tokens)),
                Cell::from(format_tokens_exact(w.planner_steps)),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(15),
            Constraint::Length(14),
            Constraint::Length(9),
            Constraint::Length(11),
            Constraint::Length(9),
            Constraint::Min(7),
        ],
    )
    .header(header)
    .block(block)
    .column_spacing(1);

    frame.render_widget(table, area);
}

fn render_model_table(frame: &mut Frame, area: Rect, app: &TuiAppState<'_>, compact: bool) {
    let snap = app.snapshot;
    let title = if compact {
        " MODEL BREAKDOWN (/usage POOL QUOTAS + LOCAL TRANSCRIPT TOKENS) "
    } else {
        " AVAILABLE MODELS & PER-MODEL QUOTA / USAGE "
    };
    let block = Block::default()
        .title(Span::styled(title, theme::title()))
        .borders(Borders::ALL)
        .border_style(theme::border())
        .style(theme::base());

    let header = Row::new([
        Cell::from("Model"),
        Cell::from("Status"),
        Cell::from("5h Left"),
        Cell::from("Wk Left"),
        Cell::from("Reset"),
        Cell::from("5h/All Steps"),
        Cell::from("Input"),
        Cell::from("Cache"),
        Cell::from("Output"),
    ])
    .style(
        Style::default()
            .fg(theme::BRIGHT_TEAL)
            .add_modifier(Modifier::BOLD),
    );

    let rows: Vec<Row> = snap
        .models
        .iter()
        .map(|m| {
            let name = if m.is_active {
                format!("★ {}", m.display_name)
            } else {
                format!("  {}", m.display_name)
            };
            let status_cell = if m.available {
                Cell::from("Available").style(theme::ok_style())
            } else {
                Cell::from("Historical").style(theme::muted())
            };
            let fh_cell = fmt_pct_cell(m.five_hour_pct, app.cfg);
            let wk_cell = fmt_pct_cell(m.weekly_pct, app.cfg);
            let rst_cell = Cell::from(
                m.reset_in_text
                    .as_deref()
                    .map(|s| if s == "Quota available" { "Ready" } else { s })
                    .unwrap_or("—")
                    .to_string(),
            )
            .style(theme::muted());

            Row::new([
                Cell::from(name),
                status_cell,
                fh_cell,
                wk_cell,
                rst_cell,
                Cell::from(format!(
                    "{}/{}",
                    m.steps_5h,
                    format_tokens_exact(m.steps_all_time)
                )),
                Cell::from(format_tokens(m.input_tokens)),
                Cell::from(format_tokens(m.cache_read_tokens)).style(theme::muted()),
                Cell::from(format_tokens(m.output_tokens)),
            ])
        })
        .collect();

    let mut state = TableState::default();
    if !rows.is_empty() {
        state.select(Some(app.table_scroll.min(rows.len() - 1)));
    }

    let table = Table::new(
        rows,
        [
            Constraint::Min(25),
            Constraint::Length(10),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(9),
            Constraint::Length(12),
            Constraint::Length(8),
            Constraint::Length(9),
            Constraint::Length(8),
        ],
    )
    .header(header)
    .block(block)
    .row_highlight_style(theme::highlight_row())
    .column_spacing(1);

    frame.render_stateful_widget(table, area, &mut state);
}

fn fmt_pct_cell(pct: Option<f64>, cfg: &AppConfig) -> Cell<'static> {
    match pct {
        Some(v) => {
            let st = if v < cfg.waybar.critical_remaining_pct {
                theme::crit_style()
            } else if v < cfg.waybar.warning_remaining_pct {
                theme::warn_style()
            } else {
                theme::ok_style()
            };
            Cell::from(format!("{v:.1}%")).style(st)
        }
        None => Cell::from("N/A").style(theme::muted()),
    }
}

fn render_models_tab(frame: &mut Frame, area: Rect, app: &TuiAppState<'_>) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(4)])
        .split(area);

    render_model_table(frame, chunks[0], app, false);

    let note = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("Note: ", theme::warn_style()),
            Span::raw(
                "★ indicates the active model in settings.json. Availability comes from `agy models`; 5h/Weekly % comes from `agy /usage`.",
            ),
        ]),
        Line::from(vec![
            Span::styled("Shared pools: ", theme::muted()),
            Span::raw(
                "Gemini 4 Hydrogen & 3.8 Flash share the Gemini pool; Claude & GPT-OSS share the Claude/GPT pool; Gemini 4 Argon has a separate weekly limit on Pro.",
            ),
        ]),
    ])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(theme::border())
            .title(Span::styled(" MODEL QUOTA POOL NOTES ", theme::title())),
    )
    .wrap(Wrap { trim: true });

    frame.render_widget(note, chunks[1]);
}

fn render_history_tab(frame: &mut Frame, area: Rect, app: &TuiAppState<'_>) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(12), // Interactive GitHub-style green dots heatmap
            Constraint::Length(8),  // Daily bar chart
            Constraint::Min(6),     // Daily & Weekly tables
        ])
        .split(area);

    let hm_title = format!(
        " ANTIGRAVITY USAGE HEATMAP (GREEN DOTS) — [G] {} ",
        app.heatmap_metric.label()
    );
    render_ratatui_heatmap(
        frame,
        chunks[0],
        app.daily_history,
        app.heatmap_metric,
        Some(app.heatmap_back_offset),
        &hm_title,
        false,
    );

    let bar_items: Vec<Bar> = app
        .daily_history
        .iter()
        .rev()
        .take(14)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|d| {
            let fresh_k = (d.input_tokens + d.output_tokens) / 1000;
            let short_day = d.day_local.get(5..).unwrap_or(&d.day_local).to_string();
            Bar::default()
                .value(fresh_k)
                .label(Line::from(short_day))
                .text_value(format!("{fresh_k}K"))
                .style(Style::default().fg(theme::ACCENT))
        })
        .collect();

    let chart = BarChart::default()
        .block(
            Block::default()
                .title(Span::styled(
                    " DAILY FRESH TOKENS (INPUT + OUTPUT, IN THOUSANDS) ",
                    theme::title(),
                ))
                .borders(Borders::ALL)
                .border_style(theme::border()),
        )
        .bar_width(6)
        .bar_gap(1)
        .value_style(Style::default().fg(theme::FG))
        .label_style(theme::muted())
        .data(BarGroup::default().bars(&bar_items));
    frame.render_widget(chart, chunks[1]);

    let bottom = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(chunks[2]);

    let d_header = Row::new(["Day", "Steps", "Input", "Cache Read", "Output", "Convs"]).style(
        Style::default()
            .fg(theme::BRIGHT_TEAL)
            .add_modifier(Modifier::BOLD),
    );
    let d_rows: Vec<Row> = app
        .daily_history
        .iter()
        .rev()
        .map(|d| {
            Row::new([
                Cell::from(d.day_local.clone()),
                Cell::from(format_tokens_exact(d.planner_steps)),
                Cell::from(format_tokens(d.input_tokens)),
                Cell::from(format_tokens(d.cache_read_tokens)).style(theme::muted()),
                Cell::from(format_tokens(d.output_tokens)),
                Cell::from(d.conversations_active.to_string()),
            ])
        })
        .collect();
    let mut d_state = TableState::default();
    if !d_rows.is_empty() {
        d_state.select(Some(app.table_scroll.min(d_rows.len() - 1)));
    }
    let d_table = Table::new(
        d_rows,
        [
            Constraint::Length(11),
            Constraint::Length(7),
            Constraint::Length(8),
            Constraint::Length(10),
            Constraint::Length(8),
            Constraint::Min(5),
        ],
    )
    .header(d_header)
    .row_highlight_style(theme::highlight_row())
    .block(
        Block::default()
            .title(Span::styled(" DAILY SUMMARIES (SQLITE) ", theme::title()))
            .borders(Borders::ALL)
            .border_style(theme::border()),
    );
    frame.render_stateful_widget(d_table, bottom[0], &mut d_state);

    let w_header = Row::new(["Week", "Days", "Steps", "Fresh", "Cache Read"]).style(
        Style::default()
            .fg(theme::BRIGHT_TEAL)
            .add_modifier(Modifier::BOLD),
    );
    let w_rows: Vec<Row> = app
        .weekly_history
        .iter()
        .rev()
        .map(|w| {
            Row::new([
                Cell::from(w.iso_week.clone()),
                Cell::from(w.active_days.to_string()),
                Cell::from(format_tokens_exact(w.planner_steps)),
                Cell::from(format_tokens(w.input_tokens + w.output_tokens)),
                Cell::from(format_tokens(w.cache_read_tokens)).style(theme::muted()),
            ])
        })
        .collect();
    let w_table = Table::new(
        w_rows,
        [
            Constraint::Length(9),
            Constraint::Length(5),
            Constraint::Length(7),
            Constraint::Length(9),
            Constraint::Min(9),
        ],
    )
    .header(w_header)
    .block(
        Block::default()
            .title(Span::styled(
                format!(
                    " WEEKLY SUMMARIES ({} SNAPSHOTS SAVED) ",
                    app.snapshot_history.len()
                ),
                theme::title(),
            ))
            .borders(Borders::ALL)
            .border_style(theme::border()),
    );
    frame.render_widget(w_table, bottom[1]);
}

fn render_tokens_tab(frame: &mut Frame, area: Rect, app: &TuiAppState<'_>) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(9), Constraint::Min(6)])
        .split(area);

    let all = &app.snapshot.window_all_time;
    let w5 = &app.snapshot.window_5h;
    let cache_ratio = if all.input_tokens + all.cache_read_tokens > 0 {
        (all.cache_read_tokens as f64 / (all.input_tokens + all.cache_read_tokens) as f64) * 100.0
    } else {
        0.0
    };

    let stats_lines = vec![
        Line::from(vec![
            Span::styled("Actual All-Time Input:      ", theme::muted()),
            Span::styled(
                format_tokens_exact(all.input_tokens),
                Style::default().fg(theme::FG),
            ),
            Span::styled("   │ 5h Input:   ", theme::muted()),
            Span::raw(format_tokens_exact(w5.input_tokens)),
        ]),
        Line::from(vec![
            Span::styled("Actual Cache Read Tokens:   ", theme::muted()),
            Span::styled(
                format_tokens_exact(all.cache_read_tokens),
                Style::default().fg(theme::BRIGHT_TEAL),
            ),
            Span::styled(format!(" ({cache_ratio:.1}% cache hit) │ 5h: "), theme::muted()),
            Span::raw(format_tokens_exact(w5.cache_read_tokens)),
        ]),
        Line::from(vec![
            Span::styled("Actual Output Tokens:       ", theme::muted()),
            Span::styled(
                format_tokens_exact(all.output_tokens),
                Style::default().fg(theme::FG),
            ),
            Span::styled(
                format!(
                    " (Think: {} + Text: {})",
                    format_tokens(all.thinking_tokens),
                    format_tokens(all.response_tokens)
                ),
                theme::muted(),
            ),
        ]),
        Line::from(vec![
            Span::styled("Fresh Total (In + Out):     ", theme::muted()),
            Span::styled(
                format_tokens_exact(all.fresh_total_tokens),
                Style::default()
                    .fg(theme::GREEN)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("   │ Combined (+Cache): ", theme::muted()),
            Span::raw(format_tokens_exact(all.combined_total_tokens)),
        ]),
        Line::from(vec![
            Span::styled("Remaining Token Allowance:  ", theme::muted()),
            Span::styled("Unavailable", theme::warn_style()),
            Span::styled(
                " (Antigravity reports percentage compute quotas in `/usage`, not raw token balances)",
                theme::muted(),
            ),
        ]),
    ];

    let summary = Paragraph::new(stats_lines)
        .block(
            Block::default()
                .title(Span::styled(" ACTUAL TOKEN STATISTICS ", theme::title()))
                .borders(Borders::ALL)
                .border_style(theme::border()),
        )
        .wrap(Wrap { trim: true });
    frame.render_widget(summary, chunks[0]);

    let c_header = Row::new([
        "Conversation",
        "Primary Model",
        "Steps",
        "Fresh (In+Out)",
        "Cache Read",
        "Workspace",
    ])
    .style(
        Style::default()
            .fg(theme::BRIGHT_TEAL)
            .add_modifier(Modifier::BOLD),
    );

    let c_rows: Vec<Row> = app
        .snapshot
        .top_conversations
        .iter()
        .map(|c| {
            Row::new([
                Cell::from(c.title.clone()),
                Cell::from(
                    c.primary_model
                        .clone()
                        .unwrap_or_else(|| "Unknown".to_string()),
                ),
                Cell::from(c.planner_steps.to_string()),
                Cell::from(format_tokens(c.input_tokens + c.output_tokens)),
                Cell::from(format_tokens(c.cache_read_tokens)).style(theme::muted()),
                Cell::from(shorten_path_home(&c.workspace)).style(theme::muted()),
            ])
        })
        .collect();

    let mut c_state = TableState::default();
    if !c_rows.is_empty() {
        c_state.select(Some(app.table_scroll.min(c_rows.len() - 1)));
    }

    let c_table = Table::new(
        c_rows,
        [
            Constraint::Min(22),
            Constraint::Length(24),
            Constraint::Length(6),
            Constraint::Length(14),
            Constraint::Length(11),
            Constraint::Min(18),
        ],
    )
    .header(c_header)
    .row_highlight_style(theme::highlight_row())
    .block(
        Block::default()
            .title(Span::styled(
                " TOP CONVERSATIONS BY TOKEN USAGE ",
                theme::title(),
            ))
            .borders(Borders::ALL)
            .border_style(theme::border()),
    );
    frame.render_stateful_widget(c_table, chunks[1], &mut c_state);
}

fn render_doctor_tab(frame: &mut Frame, area: Rect, app: &TuiAppState<'_>) {
    let mut lines = Vec::new();
    if let Some(report) = app.doctor_report {
        for chk in &report.checks {
            let (tag, st) = match chk.status {
                CheckSeverity::Ok => ("[OK]  ", theme::ok_style()),
                CheckSeverity::Warn => ("[WARN]", theme::warn_style()),
                CheckSeverity::Error => ("[ERR] ", theme::crit_style()),
            };
            lines.push(Line::from(vec![
                Span::styled(tag, st),
                Span::raw(" "),
                Span::styled(
                    format!("{}: ", chk.name),
                    Style::default().fg(theme::FG).add_modifier(Modifier::BOLD),
                ),
                Span::raw(&chk.detail),
            ]));
        }
    } else {
        lines.push(Line::from(Span::styled(
            "Running doctor diagnostics...",
            theme::muted(),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("Config:   ", theme::muted()),
        Span::raw(app.paths.config_file.display().to_string()),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Cache:    ", theme::muted()),
        Span::raw(app.paths.snapshot_cache.display().to_string()),
    ]));
    lines.push(Line::from(vec![
        Span::styled("History:  ", theme::muted()),
        Span::raw(app.paths.history_db.display().to_string()),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Sources:  ", theme::muted()),
        Span::raw(app.snapshot.sources_checked.join(", ")),
    ]));

    let p = Paragraph::new(lines)
        .block(
            Block::default()
                .title(Span::styled(
                    " DIAGNOSTICS & DATA PROVENANCE ",
                    theme::title(),
                ))
                .borders(Borders::ALL)
                .border_style(theme::border()),
        )
        .wrap(Wrap { trim: true });
    frame.render_widget(p, area);
}

fn render_footer(frame: &mut Frame, area: Rect, app: &TuiAppState<'_>) {
    let mut spans = vec![
        Span::styled(
            " [R]",
            Style::default()
                .fg(theme::BRIGHT_TEAL)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" Refresh "),
        Span::styled(
            "[G]",
            Style::default()
                .fg(theme::BRIGHT_TEAL)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" Dot Metric "),
        Span::styled(
            "[U]",
            Style::default()
                .fg(theme::BRIGHT_TEAL)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" agy "),
        Span::styled(
            "[M]",
            Style::default()
                .fg(theme::BRIGHT_TEAL)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" Models "),
        Span::styled(
            "[H]",
            Style::default()
                .fg(theme::BRIGHT_TEAL)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" History "),
        Span::styled(
            "[T]",
            Style::default()
                .fg(theme::BRIGHT_TEAL)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" Tokens "),
        Span::styled(
            "[X]",
            Style::default()
                .fg(theme::BRIGHT_TEAL)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" Doctor "),
        Span::styled(
            "[?]",
            Style::default()
                .fg(theme::BRIGHT_TEAL)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" Help "),
        Span::styled(
            "[Q]",
            Style::default()
                .fg(theme::BRIGHT_TEAL)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" Quit"),
    ];
    if let Some(msg) = app.status_banner {
        spans.push(Span::raw("  │ "));
        spans.push(Span::styled(msg, theme::warn_style()));
    }

    let p = Paragraph::new(Line::from(spans)).block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(theme::border()),
    );
    frame.render_widget(p, area);
}

fn render_help_overlay(frame: &mut Frame, area: Rect) {
    let w = area.width.min(72);
    let h = area.height.min(18);
    let rect = Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };

    let help = Paragraph::new(vec![
        Line::from(Span::styled("Keyboard Shortcuts", theme::title())),
        Line::from(""),
        Line::from("  1 / D       Dashboard overview (/usage pools + green-dot heatmap)"),
        Line::from("  2 / M       Available models & per-model pool quotas"),
        Line::from("  3 / H       Interactive green-dot heatmap + SQLite daily/weekly history"),
        Line::from("  4 / T       Detailed token & conversation statistics"),
        Line::from("  5 / X       Doctor checks & data provenance"),
        Line::from("  Tab / ← →   Cycle tabs"),
        Line::from("  ↑↓ / j k    Scroll active table"),
        Line::from("  G           Cycle green-dot metric (Fresh / Combined / Steps)"),
        Line::from("  [ / ]       Move heatmap day cursor back / forward (History tab)"),
        Line::from("  R           Trigger live refresh (`agy models` + `/usage`)"),
        Line::from("  U           Launch `agy` in Kitty"),
        Line::from("  ? / Esc     Toggle / close help"),
        Line::from("  Q / Ctrl+C  Quit"),
    ])
    .block(
        Block::default()
            .title(Span::styled(" HELP ", theme::title()))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme::BRIGHT_TEAL))
            .style(theme::surface()),
    )
    .wrap(Wrap { trim: true });

    frame.render_widget(Clear, rect);
    frame.render_widget(help, rect);
}
