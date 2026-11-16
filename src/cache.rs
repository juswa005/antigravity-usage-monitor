use crate::models::{ActiveStatusLine, UsageSnapshot};
use chrono::{DateTime, Utc};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub fn load_snapshot_cache(path: &Path) -> Option<UsageSnapshot> {
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str::<UsageSnapshot>(&raw).ok()
}

pub fn save_snapshot_cache(path: &Path, snapshot: &UsageSnapshot) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(snapshot)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    fs::write(&tmp, json)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

pub fn load_active_statusline(path: &Path, max_age_secs: i64) -> Option<ActiveStatusLine> {
    let raw = fs::read_to_string(path).ok()?;
    let item = serde_json::from_str::<ActiveStatusLine>(&raw).ok()?;
    if (Utc::now() - item.recorded_at).num_seconds() <= max_age_secs {
        Some(item)
    } else {
        None
    }
}

pub fn save_active_statusline(path: &Path, item: &ActiveStatusLine) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_string(item)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    fs::write(&tmp, json)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

pub fn is_live_cache_fresh(
    snapshot: &UsageSnapshot,
    refresh_interval_secs: u64,
    now: DateTime<Utc>,
) -> bool {
    let Some(live_at) = snapshot.last_live_refresh_at else {
        return false;
    };
    let age = (now - live_at).num_seconds();
    age >= 0 && (age as u64) < refresh_interval_secs
}

pub struct RefreshLockGuard {
    path: PathBuf,
}

impl Drop for RefreshLockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Acquires a non-blocking lockfile to ensure concurrent Waybar/CLI calls do not flood `agy`.
/// Stale locks older than `stale_after` are automatically cleared.
pub fn try_acquire_refresh_lock(
    lock_path: &Path,
    stale_after: Duration,
) -> Option<RefreshLockGuard> {
    if let Some(parent) = lock_path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    if let Ok(meta) = fs::metadata(lock_path) {
        if let Ok(modified) = meta.modified() {
            if let Ok(elapsed) = SystemTime::now().duration_since(modified) {
                if elapsed > stale_after {
                    let _ = fs::remove_file(lock_path);
                }
            }
        }
    }

    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(lock_path)
    {
        Ok(mut f) => {
            let _ = writeln!(f, "pid={}", std::process::id());
            Some(RefreshLockGuard {
                path: lock_path.to_path_buf(),
            })
        }
        Err(_) => None,
    }
}
