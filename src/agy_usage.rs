use crate::models::{ConnectivityState, QuotaPool, QuotaWindow};
use crate::utils::resolve_executable;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde_json::Value;
use std::fs;
use std::io;
use std::os::unix::io::RawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct AgyUsageProbe {
    pub connectivity: ConnectivityState,
    pub email: Option<String>,
    pub plan_tier: Option<String>,
    pub pools: Vec<QuotaPool>,
    pub error: Option<String>,
    pub auth_error: Option<String>,
}

pub fn probe_agy_usage(binary: &str, antigravity_dir: &Path, timeout_secs: u64) -> AgyUsageProbe {
    let cwd = resolve_trusted_cwd(antigravity_dir);
    let timeout = Duration::from_secs(timeout_secs.clamp(5, 30));

    match run_usage_pty(binary, &cwd, timeout) {
        Ok(screen_text) => {
            let low = screen_text.to_lowercase();
            if low.contains("not logged into antigravity")
                || low.contains("verify your account")
                || low.contains("permission_denied")
            {
                let msg = "Antigravity authentication required or account verification needed."
                    .to_string();
                return AgyUsageProbe {
                    connectivity: ConnectivityState::Error,
                    email: None,
                    plan_tier: None,
                    pools: Vec::new(),
                    error: Some(msg.clone()),
                    auth_error: Some(msg),
                };
            }
            if low.contains("network is unreachable")
                || low.contains("connection reset")
                || low.contains("dial tcp")
                || low.contains("failed to load quota")
            {
                return AgyUsageProbe {
                    connectivity: ConnectivityState::Offline,
                    email: None,
                    plan_tier: None,
                    pools: Vec::new(),
                    error: Some("Network error loading /usage quota summary".to_string()),
                    auth_error: None,
                };
            }

            let parsed = parse_usage_screen(&screen_text, Utc::now());
            if parsed.pools.is_empty() {
                AgyUsageProbe {
                    connectivity: ConnectivityState::Error,
                    email: parsed.email,
                    plan_tier: parsed.plan_tier,
                    pools: Vec::new(),
                    error: Some("Could not parse quota pools from `agy /usage` output".to_string()),
                    auth_error: None,
                }
            } else {
                AgyUsageProbe {
                    connectivity: ConnectivityState::Online,
                    email: parsed.email,
                    plan_tier: parsed.plan_tier,
                    pools: parsed.pools,
                    error: None,
                    auth_error: None,
                }
            }
        }
        Err(PtyErr::NotFound(msg)) => AgyUsageProbe {
            connectivity: ConnectivityState::CliMissing,
            email: None,
            plan_tier: None,
            pools: Vec::new(),
            error: Some(msg),
            auth_error: None,
        },
        Err(PtyErr::Timeout(msg)) | Err(PtyErr::Other(msg)) => AgyUsageProbe {
            connectivity: ConnectivityState::Error,
            email: None,
            plan_tier: None,
            pools: Vec::new(),
            error: Some(msg),
            auth_error: None,
        },
    }
}

enum PtyErr {
    NotFound(String),
    Timeout(String),
    Other(String),
}

struct PtyChildGuard {
    child: Child,
    master_fd: RawFd,
}

impl Drop for PtyChildGuard {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let pid = self.child.id() as libc::pid_t;
            unsafe {
                libc::kill(-pid, libc::SIGTERM);
            }
            std::thread::sleep(Duration::from_millis(120));
            if self.child.try_wait().ok().flatten().is_none() {
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                }
                let _ = self.child.wait();
            }
        }
        unsafe {
            libc::close(self.master_fd);
        }
    }
}

fn resolve_trusted_cwd(antigravity_dir: &Path) -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let settings_path = antigravity_dir.join("settings.json");
    if let Ok(raw) = fs::read_to_string(settings_path) {
        if let Ok(val) = serde_json::from_str::<Value>(&raw) {
            if let Some(arr) = val.get("trustedWorkspaces").and_then(|v| v.as_array()) {
                // Prefer $HOME or any existing workspace other than active project if available
                let paths: Vec<PathBuf> = arr
                    .iter()
                    .filter_map(|v| v.as_str())
                    .map(PathBuf::from)
                    .filter(|p| p.is_dir())
                    .collect();
                if let Some(h) = paths.iter().find(|p| *p == &home) {
                    return h.clone();
                }
                if let Some(first) = paths.first() {
                    return first.clone();
                }
            }
        }
    }
    home
}

fn run_usage_pty(binary: &str, cwd: &Path, timeout: Duration) -> Result<String, PtyErr> {
    let mut master: libc::c_int = -1;
    let mut slave: libc::c_int = -1;
    let ws = libc::winsize {
        ws_row: 100,
        ws_col: 140,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &ws,
        )
    };
    if rc != 0 {
        return Err(PtyErr::Other(format!(
            "openpty failed: {}",
            io::Error::last_os_error()
        )));
    }

    let resolved = resolve_executable(binary).unwrap_or_else(|| PathBuf::from(binary));
    let mut cmd = Command::new(&resolved);
    cmd.current_dir(cwd);
    unsafe {
        cmd.pre_exec(move || {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            if libc::ioctl(slave, libc::TIOCSCTTY as _, 0) == -1 {
                return Err(io::Error::last_os_error());
            }
            libc::dup2(slave, libc::STDIN_FILENO);
            libc::dup2(slave, libc::STDOUT_FILENO);
            libc::dup2(slave, libc::STDERR_FILENO);
            if slave > 2 {
                libc::close(slave);
            }
            if master > 2 {
                libc::close(master);
            }
            Ok(())
        });
    }

    let child = match cmd.spawn() {
        Ok(c) => {
            unsafe {
                libc::close(slave);
            }
            c
        }
        Err(e) => {
            unsafe {
                libc::close(slave);
                libc::close(master);
            }
            if e.kind() == io::ErrorKind::NotFound {
                return Err(PtyErr::NotFound(format!(
                    "Antigravity CLI `{binary}` not found in PATH"
                )));
            }
            return Err(PtyErr::Other(format!("Failed to spawn `{binary}`: {e}")));
        }
    };

    // Set master non-blocking for responsive polling
    unsafe {
        let flags = libc::fcntl(master, libc::F_GETFL);
        if flags >= 0 {
            libc::fcntl(master, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }

    let mut guard = PtyChildGuard {
        child,
        master_fd: master,
    };

    let mut parser = vt100::Parser::new(100, 140, 0);
    let mut read_buf = [0u8; 16384];
    let start = Instant::now();
    let deadline = start + timeout;

    let mut sent_usage = false;
    let mut usage_sent_at: Option<Instant> = None;
    let mut ready_to_quit_at: Option<Instant> = None;
    let mut sent_quit = false;
    let mut captured_screen = String::new();

    loop {
        if Instant::now() >= deadline {
            if !captured_screen.is_empty() {
                return Ok(captured_screen);
            }
            let txt = parser.screen().contents();
            if txt.contains("Models & Quota") && txt.contains('%') {
                return Ok(txt);
            }
            return Err(PtyErr::Timeout(format!(
                "`{binary} /usage` timed out after {}s",
                timeout.as_secs()
            )));
        }

        let mut pfd = libc::pollfd {
            fd: guard.master_fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let poll_rc = unsafe { libc::poll(&mut pfd, 1, 100) };
        if poll_rc > 0 && (pfd.revents & libc::POLLIN) != 0 {
            loop {
                let n = unsafe {
                    libc::read(
                        guard.master_fd,
                        read_buf.as_mut_ptr() as *mut libc::c_void,
                        read_buf.len(),
                    )
                };
                if n > 0 {
                    parser.process(&read_buf[..n as usize]);
                } else {
                    break;
                }
            }
        }

        let txt = parser.screen().contents();
        let low = txt.to_lowercase();

        if !sent_usage {
            if low.contains("not logged into antigravity")
                || low.contains("verify your account")
                || (low.contains("trust") && low.contains("workspace"))
            {
                return Ok(txt);
            }
            if txt.contains("for shortcuts") || txt.contains("Type a message") {
                std::thread::sleep(Duration::from_millis(250));
                write_all_fd(guard.master_fd, b"/usage\r");
                sent_usage = true;
                usage_sent_at = Some(Instant::now());
            }
        } else if !sent_quit {
            let has_panel = txt.contains("Models & Quota");
            let still_loading = txt.contains("Loading quota summary");
            let has_pct = txt.contains('%');
            let elapsed_since_cmd = usage_sent_at.map(|t| t.elapsed()).unwrap_or_default();

            if (has_panel && !still_loading && has_pct)
                || (has_panel && has_pct && elapsed_since_cmd > Duration::from_secs(3))
            {
                if ready_to_quit_at.is_none() {
                    ready_to_quit_at = Some(Instant::now());
                }
                if ready_to_quit_at
                    .map(|t| t.elapsed() >= Duration::from_millis(350))
                    .unwrap_or(false)
                {
                    captured_screen = txt.clone();
                    write_all_fd(guard.master_fd, b"\x1b");
                    std::thread::sleep(Duration::from_millis(120));
                    write_all_fd(guard.master_fd, b"\x04");
                    std::thread::sleep(Duration::from_millis(80));
                    write_all_fd(guard.master_fd, b"\x04");
                    sent_quit = true;
                }
            } else if low.contains("network is unreachable")
                || low.contains("failed to load quota")
                || low.contains("verify your account")
            {
                captured_screen = txt;
                write_all_fd(guard.master_fd, b"\x1b\x04\x04");
                sent_quit = true;
            }
        }

        if sent_quit {
            if guard.child.try_wait().ok().flatten().is_some() {
                break;
            }
            if ready_to_quit_at
                .map(|t| t.elapsed() > Duration::from_secs(2))
                .unwrap_or(false)
            {
                break;
            }
        }
    }

    if captured_screen.is_empty() {
        captured_screen = parser.screen().contents();
    }
    Ok(captured_screen)
}

fn write_all_fd(fd: RawFd, data: &[u8]) {
    let mut written = 0;
    while written < data.len() {
        let n = unsafe {
            libc::write(
                fd,
                data[written..].as_ptr() as *const libc::c_void,
                data.len() - written,
            )
        };
        if n <= 0 {
            break;
        }
        written += n as usize;
    }
}

#[derive(Debug, Clone, Default)]
pub struct ParsedUsageScreen {
    pub email: Option<String>,
    pub plan_tier: Option<String>,
    pub pools: Vec<QuotaPool>,
}

pub fn parse_usage_screen(screen: &str, now: DateTime<Utc>) -> ParsedUsageScreen {
    let mut out = ParsedUsageScreen::default();
    let lines: Vec<String> = screen
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    // 1. Parse email and plan tier (e.g. "Account: user@example.com" and "user@example.com (Google AI Pro)")
    for line in &lines {
        if let Some(rest) = line.strip_prefix("Account:") {
            let e = rest.split_whitespace().next().unwrap_or("").trim();
            if e.contains('@') {
                out.email = Some(e.to_string());
            }
        }
        if let Some(start) = line.find("(Google AI ") {
            if let Some(end) = line[start..].find(')') {
                let tier = line[start + 1..start + end].trim();
                if !tier.is_empty() {
                    out.plan_tier = Some(tier.to_string());
                }
            }
        } else if out.plan_tier.is_none() {
            if let Some(email) = out.email.as_deref() {
                if let Some(pos) = line.find(email) {
                    let after = line[pos + email.len()..].trim();
                    if after.starts_with('(') {
                        if let Some(end) = after.find(')') {
                            let tier = after[1..end].trim();
                            if !tier.is_empty() {
                                out.plan_tier = Some(tier.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    // 2. Parse pool sections inside Models & Quota
    let start_idx = lines
        .iter()
        .position(|l| l.contains("Models & Quota"))
        .unwrap_or(0);

    let mut current_pool: Option<QuotaPool> = None;
    let mut pending_window_label: Option<String> = None;
    let mut last_parsed_window_kind: Option<bool> = None; // true = five_hour, false = weekly

    for line in &lines[start_idx..] {
        if line.starts_with("│Within each group") || line.starts_with("Within each group") {
            break;
        }
        if line.contains("Scroll") && line.contains("esc Close") {
            continue;
        }

        // Pool header detection: all-uppercase ASCII heading >= 4 chars containing letters
        if is_pool_header(line) {
            if let Some(pool) = current_pool.take() {
                if pool.five_hour.is_some() || pool.weekly.is_some() {
                    out.pools.push(pool);
                }
            }
            current_pool = Some(QuotaPool {
                pool_name: line.clone(),
                models_included: Vec::new(),
                five_hour: None,
                weekly: None,
            });
            pending_window_label = None;
            last_parsed_window_kind = None;
            continue;
        }

        let Some(ref mut pool) = current_pool else {
            continue;
        };

        if let Some(rest) = line.strip_prefix("Models within this group:") {
            pool.models_included = rest
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            continue;
        }

        if line.contains("Limit")
            && !line.contains('%')
            && !line.starts_with('[')
            && !line.starts_with("For paid")
        {
            pending_window_label = Some(line.clone());
            continue;
        }

        if let Some(pct) = extract_bar_percentage(line) {
            let label = pending_window_label
                .take()
                .unwrap_or_else(|| "Limit".to_string());
            let is_five_hour = label.to_lowercase().contains("five hour")
                || label.to_lowercase().contains("5-hour")
                || label.to_lowercase().contains("5h");
            let win = QuotaWindow {
                window_label: label,
                remaining_pct: pct,
                used_pct: (100.0 - pct).clamp(0.0, 100.0),
                refresh_in_text: None,
                refresh_in_secs: None,
                refreshes_at: None,
            };
            if is_five_hour {
                pool.five_hour = Some(win);
            } else {
                pool.weekly = Some(win);
            }
            last_parsed_window_kind = Some(is_five_hour);
            continue;
        }

        if line.starts_with("Refreshes in ") || line == "Quota available" {
            let target = match last_parsed_window_kind {
                Some(true) => pool.five_hour.as_mut(),
                Some(false) => pool.weekly.as_mut(),
                None => None,
            };
            if let Some(win) = target {
                if let Some(rest) = line.strip_prefix("Refreshes in ") {
                    let clean_rest = rest.trim().to_string();
                    let secs = parse_countdown_secs(&clean_rest);
                    win.refresh_in_text = Some(clean_rest);
                    win.refresh_in_secs = secs;
                    win.refreshes_at = secs.map(|s| now + ChronoDuration::seconds(s));
                } else {
                    win.refresh_in_text = Some("Quota available".to_string());
                }
            }
        }
    }

    if let Some(pool) = current_pool.take() {
        if pool.five_hour.is_some() || pool.weekly.is_some() {
            out.pools.push(pool);
        }
    }

    out
}

fn is_pool_header(line: &str) -> bool {
    if line.len() < 4 || line.contains('%') || line.contains(':') || line.starts_with('[') {
        return false;
    }
    let has_alpha = line.chars().any(|c| c.is_ascii_alphabetic());
    let all_upper_or_punct = line
        .chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == ' ' || c == '&' || c == '-');
    has_alpha && all_upper_or_punct
}

fn extract_bar_percentage(line: &str) -> Option<f64> {
    let pct_pos = line.find('%')?;
    let before = line[..pct_pos].trim_end();
    let num_str = before
        .rsplit(|c: char| !c.is_ascii_digit() && c != '.')
        .next()?;
    let val: f64 = num_str.parse().ok()?;
    if (0.0..=100.0).contains(&val) {
        Some(val)
    } else {
        None
    }
}

pub fn parse_countdown_secs(text: &str) -> Option<i64> {
    let mut total = 0i64;
    let mut found = false;
    for tok in text.split_whitespace() {
        if let Some(num) = tok.strip_suffix('d') {
            if let Ok(v) = num.parse::<i64>() {
                total += v * 86_400;
                found = true;
            }
        } else if let Some(num) = tok.strip_suffix('h') {
            if let Ok(v) = num.parse::<i64>() {
                total += v * 3_600;
                found = true;
            }
        } else if let Some(num) = tok.strip_suffix('m') {
            if let Ok(v) = num.parse::<i64>() {
                total += v * 60;
                found = true;
            }
        } else if let Some(num) = tok.strip_suffix('s') {
            if let Ok(v) = num.parse::<i64>() {
                total += v;
                found = true;
            }
        }
    }
    if found {
        Some(total)
    } else {
        None
    }
}
