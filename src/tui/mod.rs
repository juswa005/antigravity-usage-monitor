pub mod heatmap;
pub mod theme;
pub mod ui;

use crate::collector::{collect_usage, CollectOptions};
use crate::config::{AppConfig, AppPaths};
use crate::database::HistoryDatabase;
use crate::doctor::run_doctor;
use crate::models::{
    DailyTokenBucket, DoctorReport, SnapshotHistoryRow, UsageSnapshot, WeeklySummaryRow,
};
use crate::utils::format_local_time;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use heatmap::HeatmapMetric;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use ui::{ActiveTab, TuiAppState};

enum BgResult {
    Refreshed(Box<UsageSnapshot>),
    Doctor(Box<DoctorReport>),
}

pub fn run_tui(cfg: AppConfig, paths: AppPaths) -> io::Result<()> {
    let mut snapshot = collect_usage(
        &cfg,
        &paths,
        CollectOptions {
            force_refresh: false,
            cache_only: true,
        },
    );
    let (mut daily_history, mut weekly_history, mut snapshot_history) =
        load_history(&paths, cfg.history.default_days);
    let mut doctor_report: Option<DoctorReport> = None;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let (tx, rx) = mpsc::channel::<BgResult>();
    let mut refreshing = false;
    let mut status_banner: Option<String> = None;
    let mut tab = ActiveTab::Dashboard;
    let mut table_scroll = 0usize;
    let mut heatmap_metric = HeatmapMetric::FreshTokens;
    let mut heatmap_back_offset = 0usize;
    let mut show_help = false;

    if snapshot.stale || snapshot.last_live_refresh_at.is_none() || !snapshot.quota.remote_available
    {
        refreshing = true;
        spawn_refresh(tx.clone(), cfg.clone(), paths.clone(), false);
    }

    let mut last_auto_tick = Instant::now();
    let res = loop {
        while let Ok(msg) = rx.try_recv() {
            match msg {
                BgResult::Refreshed(new_snap) => {
                    snapshot = *new_snap;
                    refreshing = false;
                    let (d, w, s) = load_history(&paths, cfg.history.default_days);
                    daily_history = d;
                    weekly_history = w;
                    snapshot_history = s;
                    status_banner = Some(format!(
                        "Updated at {}",
                        format_local_time(snapshot.collected_at)
                    ));
                }
                BgResult::Doctor(rep) => {
                    doctor_report = Some(*rep);
                }
            }
        }

        if last_auto_tick.elapsed()
            >= Duration::from_secs(cfg.general.refresh_interval_secs.max(30))
            && !refreshing
        {
            refreshing = true;
            spawn_refresh(tx.clone(), cfg.clone(), paths.clone(), false);
            last_auto_tick = Instant::now();
        }

        let app_state = TuiAppState {
            tab,
            snapshot: &snapshot,
            daily_history: &daily_history,
            weekly_history: &weekly_history,
            snapshot_history: &snapshot_history,
            doctor_report: doctor_report.as_ref(),
            cfg: &cfg,
            paths: &paths,
            table_scroll,
            heatmap_metric,
            heatmap_back_offset,
            refreshing,
            status_banner: status_banner.as_deref(),
            show_help,
        };
        if let Err(e) = terminal.draw(|f| ui::render(f, &app_state)) {
            break Err(e);
        }

        if event::poll(Duration::from_millis(120))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                    break Ok(());
                }

                if show_help {
                    match key.code {
                        KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') => {
                            show_help = false;
                        }
                        _ => {}
                    }
                    continue;
                }

                match key.code {
                    KeyCode::Char('q') | KeyCode::Char('Q') => break Ok(()),
                    KeyCode::Char('?') => show_help = true,
                    KeyCode::Char('1') | KeyCode::Char('d') | KeyCode::Char('D') => {
                        tab = ActiveTab::Dashboard;
                        table_scroll = 0;
                    }
                    KeyCode::Char('2') | KeyCode::Char('m') | KeyCode::Char('M') => {
                        tab = ActiveTab::Models;
                        table_scroll = 0;
                    }
                    KeyCode::Char('3') | KeyCode::Char('h') | KeyCode::Char('H') => {
                        tab = ActiveTab::History;
                        table_scroll = 0;
                    }
                    KeyCode::Char('4') | KeyCode::Char('t') | KeyCode::Char('T') => {
                        tab = ActiveTab::Tokens;
                        table_scroll = 0;
                    }
                    KeyCode::Char('5') | KeyCode::Char('x') | KeyCode::Char('X') => {
                        tab = ActiveTab::Doctor;
                        table_scroll = 0;
                        if doctor_report.is_none() {
                            spawn_doctor(tx.clone(), cfg.clone(), paths.clone());
                        }
                    }
                    KeyCode::Tab | KeyCode::Right => {
                        let next = (tab.index() + 1) % ActiveTab::ALL.len();
                        tab = ActiveTab::ALL[next];
                        table_scroll = 0;
                        if tab == ActiveTab::Doctor && doctor_report.is_none() {
                            spawn_doctor(tx.clone(), cfg.clone(), paths.clone());
                        }
                    }
                    KeyCode::BackTab | KeyCode::Left => {
                        let prev = (tab.index() + ActiveTab::ALL.len() - 1) % ActiveTab::ALL.len();
                        tab = ActiveTab::ALL[prev];
                        table_scroll = 0;
                        if tab == ActiveTab::Doctor && doctor_report.is_none() {
                            spawn_doctor(tx.clone(), cfg.clone(), paths.clone());
                        }
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        table_scroll = table_scroll.saturating_add(1);
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        table_scroll = table_scroll.saturating_sub(1);
                    }
                    KeyCode::Char('g') | KeyCode::Char('G') => {
                        heatmap_metric = heatmap_metric.next();
                        status_banner = Some(format!("Heatmap metric: {}", heatmap_metric.label()));
                    }
                    KeyCode::Char('[') | KeyCode::Char('p') => {
                        heatmap_back_offset = heatmap_back_offset.saturating_add(1).min(365);
                    }
                    KeyCode::Char(']') | KeyCode::Char('n') => {
                        heatmap_back_offset = heatmap_back_offset.saturating_sub(1);
                    }
                    KeyCode::Char('r') | KeyCode::Char('R') => {
                        if !refreshing {
                            refreshing = true;
                            status_banner =
                                Some("Refreshing via `agy models` + `/usage`...".to_string());
                            spawn_refresh(tx.clone(), cfg.clone(), paths.clone(), true);
                            if tab == ActiveTab::Doctor {
                                spawn_doctor(tx.clone(), cfg.clone(), paths.clone());
                            }
                        }
                    }
                    KeyCode::Char('u') | KeyCode::Char('U') => match launch_agy_for_usage(&cfg) {
                        Ok(msg) => status_banner = Some(msg),
                        Err(e) => status_banner = Some(format!("Failed to launch agy: {e}")),
                    },
                    _ => {}
                }
            }
        }
    };

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    res
}

fn spawn_refresh(tx: mpsc::Sender<BgResult>, cfg: AppConfig, paths: AppPaths, force: bool) {
    std::thread::spawn(move || {
        let snap = collect_usage(
            &cfg,
            &paths,
            CollectOptions {
                force_refresh: force,
                cache_only: false,
            },
        );
        let _ = tx.send(BgResult::Refreshed(Box::new(snap)));
    });
}

fn spawn_doctor(tx: mpsc::Sender<BgResult>, cfg: AppConfig, paths: AppPaths) {
    std::thread::spawn(move || {
        let report = run_doctor(&cfg, &paths);
        let _ = tx.send(BgResult::Doctor(Box::new(report)));
    });
}

fn load_history(
    paths: &AppPaths,
    days: usize,
) -> (
    Vec<DailyTokenBucket>,
    Vec<WeeklySummaryRow>,
    Vec<SnapshotHistoryRow>,
) {
    let Ok(db) = HistoryDatabase::open(&paths.history_db) else {
        return (Vec::new(), Vec::new(), Vec::new());
    };
    let d = db.query_daily_summaries(days.max(400)).unwrap_or_default();
    let w = db.query_weekly_summaries(16).unwrap_or_default();
    let s = db.query_recent_snapshots(30).unwrap_or_default();
    (d, w, s)
}

fn launch_agy_for_usage(cfg: &AppConfig) -> io::Result<String> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some() {
        Command::new("sh")
            .arg("-c")
            .arg(&cfg.waybar.agy_launch_command)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok("Opened `agy` in new Kitty window — type /usage or /credits".to_string())
    } else {
        Err(io::Error::new(
            io::ErrorKind::NotConnected,
            "No GUI display detected; run `agy` directly and type /usage",
        ))
    }
}
