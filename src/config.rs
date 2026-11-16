use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralConfig {
    pub agy_binary: String,
    pub antigravity_dir: Option<PathBuf>,
    pub refresh_interval_secs: u64,
    pub cli_timeout_secs: u64,
    pub fetch_tui_usage: bool,
    pub usage_timeout_secs: u64,
    pub mask_email: bool,
    pub local_5h_token_budget: Option<u64>,
    pub budget_include_cache_read: bool,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            agy_binary: "agy".to_string(),
            antigravity_dir: None,
            refresh_interval_secs: 300,
            cli_timeout_secs: 10,
            fetch_tui_usage: true,
            usage_timeout_secs: 15,
            mask_email: true,
            local_5h_token_budget: None,
            budget_include_cache_read: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WaybarConfig {
    pub display_mode: String,
    pub warning_remaining_pct: f64,
    pub critical_remaining_pct: f64,
    pub signal: u32,
    pub terminal_command: String,
    pub agy_launch_command: String,
}

impl Default for WaybarConfig {
    fn default() -> Self {
        Self {
            display_mode: "quota".to_string(),
            warning_remaining_pct: 30.0,
            critical_remaining_pct: 10.0,
            signal: 10,
            terminal_command: "kitty --class agmonitor -e agmonitor".to_string(),
            agy_launch_command: "kitty --class waybar-kitty -e agy".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HistoryConfig {
    pub default_days: usize,
    pub max_snapshots: usize,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            default_days: 14,
            max_snapshots: 5000,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub general: GeneralConfig,
    pub waybar: WaybarConfig,
    pub history: HistoryConfig,
}

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub config_file: PathBuf,
    pub cache_dir: PathBuf,
    pub snapshot_cache: PathBuf,
    pub statusline_cache: PathBuf,
    pub refresh_lock: PathBuf,
    pub data_dir: PathBuf,
    pub history_db: PathBuf,
    pub antigravity_dir: PathBuf,
}

impl AppPaths {
    pub fn resolve(cfg: &AppConfig) -> Self {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));

        let config_base = env_path("AGMONITOR_CONFIG_DIR")
            .or_else(|| env_path("XDG_CONFIG_HOME"))
            .unwrap_or_else(|| home.join(".config"));

        let cache_base = env_path("AGMONITOR_CACHE_DIR")
            .or_else(|| env_path("XDG_CACHE_HOME"))
            .unwrap_or_else(|| home.join(".cache"));

        let data_base = env_path("AGMONITOR_DATA_DIR")
            .or_else(|| env_path("XDG_DATA_HOME"))
            .unwrap_or_else(|| home.join(".local").join("share"));

        let antigravity_dir = env_path("AGMONITOR_ANTIGRAVITY_DIR")
            .or_else(|| cfg.general.antigravity_dir.clone())
            .unwrap_or_else(|| home.join(".gemini").join("antigravity-cli"));

        let config_dir = config_base.join("agmonitor");
        let cache_dir = cache_base.join("agmonitor");
        let data_dir = data_base.join("agmonitor");

        Self {
            config_file: config_dir.join("config.toml"),
            snapshot_cache: cache_dir.join("snapshot.json"),
            statusline_cache: cache_dir.join("active_statusline.json"),
            refresh_lock: cache_dir.join("refresh.lock"),
            cache_dir,
            history_db: data_dir.join("history.db"),
            data_dir,
            antigravity_dir,
        }
    }

    pub fn ensure_runtime_dirs(&self) -> std::io::Result<()> {
        fs::create_dir_all(&self.cache_dir)?;
        fs::create_dir_all(&self.data_dir)?;
        Ok(())
    }
}

fn env_path(var: &str) -> Option<PathBuf> {
    env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

pub fn load_config(explicit_path: Option<&Path>) -> (AppConfig, AppPaths) {
    let default_cfg = AppConfig::default();
    let default_paths = AppPaths::resolve(&default_cfg);
    let config_path = explicit_path
        .map(PathBuf::from)
        .or_else(|| env_path("AGMONITOR_CONFIG"))
        .unwrap_or_else(|| default_paths.config_file.clone());

    let mut cfg = match fs::read_to_string(&config_path) {
        Ok(raw) => toml::from_str::<AppConfig>(&raw).unwrap_or_default(),
        Err(_) => AppConfig::default(),
    };

    if cfg.general.refresh_interval_secs == 0 {
        cfg.general.refresh_interval_secs = 300;
    }
    if cfg.general.cli_timeout_secs == 0 {
        cfg.general.cli_timeout_secs = 10;
    }
    if cfg.general.usage_timeout_secs == 0 {
        cfg.general.usage_timeout_secs = 15;
    }
    if cfg.general.local_5h_token_budget == Some(0) {
        cfg.general.local_5h_token_budget = None;
    }

    let mut paths = AppPaths::resolve(&cfg);
    paths.config_file = config_path;
    (cfg, paths)
}
