use chrono::{DateTime, Local, Utc};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub fn resolve_executable(name: &str) -> Option<PathBuf> {
    let p = Path::new(name);
    if name.contains('/') {
        return is_executable_file(p).then(|| p.to_path_buf());
    }

    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join(name);
            if is_executable_file(&candidate) {
                return Some(candidate);
            }
        }
    }

    // Fallback locations common on Arch, Debian/Ubuntu, and user installs
    if name == "agy" {
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            for candidate in [
                home.join(".gemini/antigravity-cli/bin/agy"),
                home.join(".local/bin/agy"),
                PathBuf::from("/usr/local/bin/agy"),
                PathBuf::from("/usr/bin/agy"),
            ] {
                if is_executable_file(&candidate) {
                    return Some(candidate);
                }
            }
        }
    }

    None
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    meta.is_file() && (meta.permissions().mode() & 0o111 != 0)
}

pub fn mask_email(email: &str) -> String {
    let Some((local, domain)) = email.split_once('@') else {
        return "***".to_string();
    };
    let visible: String = local.chars().take(2).collect();
    if visible.is_empty() {
        format!("***@{domain}")
    } else {
        format!("{visible}***@{domain}")
    }
}

pub fn display_email(email: Option<&str>, mask: bool) -> String {
    match email {
        Some(e) if !e.is_empty() => {
            if mask {
                mask_email(e)
            } else {
                e.to_string()
            }
        }
        _ => "Unavailable".to_string(),
    }
}

pub fn format_tokens(n: u64) -> String {
    if n >= 1_000_000_000 {
        format!("{:.2}B", n as f64 / 1_000_000_000.0)
    } else if n >= 1_000_000 {
        format!("{:.2}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

pub fn format_tokens_exact(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (idx, ch) in s.chars().rev().enumerate() {
        if idx > 0 && idx % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

pub fn format_duration_secs(secs: i64) -> String {
    if secs <= 0 {
        return "0m".to_string();
    }
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let mins = (secs % 3_600) / 60;
    if days > 0 {
        format!("{days}d {hours:02}h")
    } else if hours > 0 {
        format!("{hours:02}h {mins:02}m")
    } else if mins > 0 {
        format!("{mins}m")
    } else {
        format!("{secs}s")
    }
}

pub fn format_age(dt: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let diff = (now - dt).num_seconds();
    if diff < 10 {
        "just now".to_string()
    } else if diff < 60 {
        format!("{diff}s ago")
    } else {
        format!("{} ago", format_duration_secs(diff))
    }
}

pub fn format_local_time(dt: DateTime<Utc>) -> String {
    dt.with_timezone(&Local).format("%H:%M:%S").to_string()
}

pub fn format_local_datetime(dt: DateTime<Utc>) -> String {
    dt.with_timezone(&Local)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

pub fn model_family_from_label(label: &str) -> String {
    if let Some((base, _)) = label.split_once(" (") {
        base.trim().to_string()
    } else {
        label.trim().to_string()
    }
}

pub fn shorten_model_label(label: &str) -> String {
    let stripped = label.replace("Gemini ", "G").replace("Claude ", "C.");
    if stripped.len() <= 20 {
        stripped
    } else {
        stripped.chars().take(19).collect::<String>() + "…"
    }
}

pub fn escape_pango(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub fn shorten_path_home(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("file://") {
        return shorten_path_home(rest);
    }
    if let Ok(home) = std::env::var("HOME") {
        if let Some(rest) = path.strip_prefix(&home) {
            return format!("~{rest}");
        }
    }
    let p = Path::new(path);
    p.to_string_lossy().into_owned()
}
