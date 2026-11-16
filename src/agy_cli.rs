use crate::models::ConnectivityState;
use crate::utils::resolve_executable;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct AgyModelEntry {
    pub id: String,
    pub display_name: String,
}

#[derive(Debug, Clone)]
pub struct AgyCliProbe {
    pub version: Option<String>,
    pub models: Vec<AgyModelEntry>,
    pub connectivity: ConnectivityState,
    pub error: Option<String>,
    pub auth_error: Option<String>,
}

pub fn probe_agy(binary: &str, timeout_secs: u64) -> AgyCliProbe {
    let timeout = Duration::from_secs(timeout_secs.max(2));
    let version = fetch_version(binary, Duration::from_secs(3));

    match fetch_models_with_retry(binary, timeout) {
        Ok(models) => AgyCliProbe {
            version,
            models,
            connectivity: ConnectivityState::Online,
            error: None,
            auth_error: None,
        },
        Err(ProbeError::CliMissing(msg)) => AgyCliProbe {
            version,
            models: Vec::new(),
            connectivity: ConnectivityState::CliMissing,
            error: Some(msg),
            auth_error: None,
        },
        Err(ProbeError::Auth(msg)) => AgyCliProbe {
            version,
            models: Vec::new(),
            connectivity: ConnectivityState::Error,
            error: Some(msg.clone()),
            auth_error: Some(msg),
        },
        Err(ProbeError::Offline(msg)) => AgyCliProbe {
            version,
            models: Vec::new(),
            connectivity: ConnectivityState::Offline,
            error: Some(msg),
            auth_error: None,
        },
        Err(ProbeError::Other(msg)) => AgyCliProbe {
            version,
            models: Vec::new(),
            connectivity: ConnectivityState::Error,
            error: Some(msg),
            auth_error: None,
        },
    }
}

#[derive(Debug)]
enum ProbeError {
    CliMissing(String),
    Auth(String),
    Offline(String),
    Other(String),
}

fn fetch_version(binary: &str, timeout: Duration) -> Option<String> {
    let out = run_with_timeout(binary, &["--version"], timeout).ok()?;
    if out.status_code != Some(0) {
        return None;
    }
    let cleaned = clean_terminal_output(&out.stdout);
    cleaned
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(ToString::to_string)
}

fn fetch_models_with_retry(
    binary: &str,
    timeout: Duration,
) -> Result<Vec<AgyModelEntry>, ProbeError> {
    match fetch_models_once(binary, timeout) {
        Ok(models) => Ok(models),
        Err(err @ ProbeError::CliMissing(_)) | Err(err @ ProbeError::Auth(_)) => Err(err),
        Err(_transient) => {
            // Single bounded retry for transient network/reset glitches
            fetch_models_once(binary, timeout)
        }
    }
}

fn fetch_models_once(binary: &str, timeout: Duration) -> Result<Vec<AgyModelEntry>, ProbeError> {
    let out = run_with_timeout(binary, &["models"], timeout)?;
    let stdout_clean = clean_terminal_output(&out.stdout);
    let stderr_clean = clean_terminal_output(&out.stderr);
    let combined_lower = format!("{stdout_clean}\n{stderr_clean}").to_lowercase();

    if combined_lower.contains("not logged into antigravity")
        || combined_lower.contains("not authenticated")
        || combined_lower.contains("unauthenticated")
        || combined_lower.contains("verify your account")
        || combined_lower.contains("permission_denied")
    {
        let snippet = first_nonempty_line(&stderr_clean)
            .or_else(|| first_nonempty_line(&stdout_clean))
            .unwrap_or_else(|| "Antigravity authentication required".to_string());
        return Err(ProbeError::Auth(snippet));
    }

    if combined_lower.contains("network is unreachable")
        || combined_lower.contains("connection reset")
        || combined_lower.contains("no such host")
        || combined_lower.contains("temporary failure in name resolution")
        || combined_lower.contains("dial tcp")
        || combined_lower.contains("context deadline exceeded")
    {
        let snippet = first_nonempty_line(&stderr_clean)
            .or_else(|| first_nonempty_line(&stdout_clean))
            .unwrap_or_else(|| "Network unreachable while querying Antigravity".to_string());
        return Err(ProbeError::Offline(snippet));
    }

    if out.status_code != Some(0) {
        let msg = first_nonempty_line(&stderr_clean)
            .or_else(|| first_nonempty_line(&stdout_clean))
            .unwrap_or_else(|| {
                format!("`{binary} models` exited with status {:?}", out.status_code)
            });
        return Err(ProbeError::Other(msg));
    }

    let models = parse_models_output(&stdout_clean);
    if models.is_empty() {
        return Err(ProbeError::Other(
            "`agy models` succeeded but returned no model entries".to_string(),
        ));
    }
    Ok(models)
}

pub fn parse_models_output(raw: &str) -> Vec<AgyModelEntry> {
    let cleaned = clean_terminal_output(raw);
    let mut entries = Vec::new();
    for line in cleaned.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.contains("Fetching available models") {
            continue;
        }
        if let Some((id, rest)) = trimmed.split_once(char::is_whitespace) {
            let display_name = rest.trim();
            if !id.is_empty() && !display_name.is_empty() && !id.starts_with('[') {
                entries.push(AgyModelEntry {
                    id: id.to_string(),
                    display_name: display_name.to_string(),
                });
            }
        }
    }
    entries
}

pub fn clean_terminal_output(raw: &str) -> String {
    // Strip ANSI escape sequences and spinner carriage-return segments
    let without_ansi = strip_ansi(raw);
    let mut lines = Vec::new();
    for line in without_ansi.lines() {
        let last_segment = line.rsplit('\r').next().unwrap_or("");
        let trimmed = last_segment.trim();
        if !trimmed.is_empty() {
            lines.push(trimmed);
        }
    }
    lines.join("\n")
}

fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if matches!(chars.peek(), Some('[')) {
                chars.next();
                for nc in chars.by_ref() {
                    if ('@'..='~').contains(&nc) {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn first_nonempty_line(s: &str) -> Option<String> {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.contains("Fetching available models"))
        .map(ToString::to_string)
}

struct CmdOutput {
    status_code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn run_with_timeout(
    binary: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<CmdOutput, ProbeError> {
    let resolved = resolve_executable(binary).unwrap_or_else(|| std::path::PathBuf::from(binary));
    let mut child = Command::new(&resolved)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ProbeError::CliMissing(format!("Antigravity CLI `{binary}` not found in PATH"))
            } else {
                ProbeError::Other(format!("Failed to launch `{binary}`: {e}"))
            }
        })?;

    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();

    let out_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(ref mut r) = stdout_pipe {
            let _ = r.read_to_end(&mut buf);
        }
        String::from_utf8_lossy(&buf).into_owned()
    });

    let err_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(ref mut r) = stderr_pipe {
            let _ = r.read_to_end(&mut buf);
        }
        String::from_utf8_lossy(&buf).into_owned()
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = out_handle.join();
                    let _ = err_handle.join();
                    return Err(ProbeError::Offline(format!(
                        "`{binary} {}` timed out after {}s",
                        args.join(" "),
                        timeout.as_secs()
                    )));
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => {
                let _ = child.kill();
                return Err(ProbeError::Other(format!("Wait error for `{binary}`: {e}")));
            }
        }
    };

    let stdout = out_handle.join().unwrap_or_default();
    let stderr = err_handle.join().unwrap_or_default();

    Ok(CmdOutput {
        status_code: status.and_then(|s| s.code()),
        stdout,
        stderr,
    })
}
