use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "agmonitor",
    version,
    about = "Antigravity Usage Monitor (AUM) — TUI dashboard, Waybar JSON module, and local usage history"
)]
pub struct Cli {
    /// Path to custom config.toml (defaults to $XDG_CONFIG_HOME/agmonitor/config.toml)
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,

    /// Show full unmasked email addresses in output
    #[arg(long, global = true)]
    pub unmask: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Open the interactive terminal dashboard (default)
    Tui,
    /// Print a concise usage summary
    Status(FormatArgs),
    /// Force a live usage refresh (`agy models` + `/usage` + local records)
    Refresh(RefreshArgs),
    /// List available models and per-model usage breakdown
    Models(FormatArgs),
    /// Display daily/weekly history and green-dot activity heatmap from SQLite
    History(HistoryArgs),
    /// Display detailed actual token consumption statistics
    Tokens(FormatArgs),
    /// Output Waybar-compatible custom module JSON
    Waybar(WaybarArgs),
    /// Check installation, authentication, connectivity, and local data sources
    Doctor(FormatArgs),
    /// Read Antigravity `statusLine` JSON from stdin and cache live session context
    StatuslineHook,
}

#[derive(Debug, Args)]
pub struct FormatArgs {
    /// Output machine-readable JSON
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct RefreshArgs {
    /// Output machine-readable JSON
    #[arg(long)]
    pub json: bool,
    /// Send SIGRTMIN+<signal> to Waybar after refreshing
    #[arg(long)]
    pub signal_waybar: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum CliHeatmapMetric {
    Fresh,
    Combined,
    Steps,
}

#[derive(Debug, Args)]
pub struct HistoryArgs {
    /// Number of recent days to display
    #[arg(long)]
    pub days: Option<usize>,
    /// Number of weeks in the green-dot heatmap (default 20)
    #[arg(long, default_value_t = 20)]
    pub weeks: usize,
    /// Metric used for green-dot intensity
    #[arg(long, value_enum, default_value_t = CliHeatmapMetric::Fresh)]
    pub metric: CliHeatmapMetric,
    /// Output machine-readable JSON
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct WaybarArgs {
    /// Force a synchronous refresh before emitting JSON (for Waybar right-click)
    #[arg(long)]
    pub refresh: bool,
    /// Never block on `agy` subprocess; emit cached JSON immediately and refresh in background if stale
    #[arg(long)]
    pub non_blocking: bool,
}
