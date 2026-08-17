use crate::config::{expand_cmd, Config, Kind, MAX_HEIGHT};
use serde_json::Value;
use std::io::Write;
use std::process::{Command, Stdio};
use unicode_width::UnicodeWidthChar;

pub const BG: &str = "\x1b[48;5;236m";
pub const FG: &str = "\x1b[38;5;252m";
pub const DIM: &str = "\x1b[38;5;245m";
pub const CYAN: &str = "\x1b[38;5;117m";
pub const GREEN: &str = "\x1b[38;5;114m";
pub const YELLOW: &str = "\x1b[38;5;221m";
pub const RED: &str = "\x1b[38;5;210m";
pub const RESET: &str = "\x1b[0m";

fn strip_ansi(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    for x in chars.by_ref() {
                        if x.is_ascii_alphabetic() {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    for x in chars.by_ref() {
                        if x == '\u{7}' {
                            break;
                        }
                    }
                }
                _ => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

pub fn vis_width(s: &str) -> usize {
    strip_ansi(s)
        .chars()
        .map(|c| UnicodeWidthChar::width(c).unwrap_or(0))
        .sum()
}

fn clip(s: &str, width: usize) -> String {
    if vis_width(s) <= width {
        return s.to_string();
    }
    let mut out = String::new();
    let mut n = 0;
    for c in strip_ansi(s).chars() {
        let w = UnicodeWidthChar::width(c).unwrap_or(0);
        if n + w > width.saturating_sub(1) {
            break;
        }
        out.push(c);
        n += w;
    }
    out.push('…');
    out
}

pub fn pad_bar(inner: &str, cols: usize, padding: usize) -> String {
    let body = format!("{}{inner}", " ".repeat(padding));
    let used = vis_width(&body);
    let body = if used > cols {
        clip(&body, cols)
    } else {
        body
    };
    let used = vis_width(&body);
    format!("{BG}{FG}{body}{}{RESET}", " ".repeat(cols.saturating_sub(used)))
}

pub fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 10_000 {
        format!("{}k", n / 1000)
    } else if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

pub fn fmt_duration(seconds: f64) -> String {
    let sec = seconds.max(0.0) as u64;
    if sec < 60 {
        return format!("{sec}s");
    }
    let mins = sec / 60;
    let rem = sec % 60;
    if mins < 60 {
        if rem == 0 {
            format!("{mins}m")
        } else {
            format!("{mins}m{rem:02}s")
        }
    } else {
        format!("{}h{:02}m", mins / 60, mins % 60)
    }
}

fn pct_color(pct: f64) -> &'static str {
    if pct >= 90.0 {
        RED
    } else if pct >= 70.0 {
        YELLOW
    } else {
        GREEN
    }
}

fn progress_bar(pct: f64, width: usize) -> String {
    let width = width.max(4);
    let filled = ((pct.clamp(0.0, 100.0) * width as f64 / 100.0).round() as usize).min(width);
    format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
}

fn jstr<'a>(v: &'a Value, path: &[&str]) -> &'a str {
    let mut cur = v;
    for p in path {
        cur = match cur.get(*p) {
            Some(x) => x,
            None => return "",
        };
    }
    cur.as_str().unwrap_or("")
}

fn ju64(v: &Value, path: &[&str]) -> u64 {
    let mut cur = v;
    for p in path {
        cur = match cur.get(*p) {
            Some(x) => x,
            None => return 0,
        };
    }
    cur.as_u64()
        .or_else(|| cur.as_f64().map(|f| f as u64))
        .unwrap_or(0)
}

fn jf64(v: &Value, path: &[&str]) -> f64 {
    let mut cur = v;
    for p in path {
        cur = match cur.get(*p) {
            Some(x) => x,
            None => return 0.0,
        };
    }
    cur.as_f64()
        .or_else(|| cur.as_u64().map(|n| n as f64))
        .unwrap_or(0.0)
}

pub fn render_builtin(payload: &Value, cols: usize, cfg: &Config) -> Vec<String> {
    let model = {
        let d = jstr(payload, &["model", "display_name"]);
        if d.is_empty() {
            jstr(payload, &["model", "id"])
        } else {
            d
        }
    };
    let model = if model.is_empty() { "grok" } else { model };
    let cwd = {
        let d = jstr(payload, &["workspace", "current_dir"]);
        if d.is_empty() {
            jstr(payload, &["cwd"])
        } else {
            d
        }
    };
    let folder = std::path::Path::new(cwd)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| cwd.to_string());
    let branch = jstr(payload, &["git", "branch"]);
    let staged = ju64(payload, &["git", "staged"]);
    let modified = ju64(payload, &["git", "modified"]);
    let effort = jstr(payload, &["effort", "level"]);

    let mut left = vec![format!("{CYAN}[{model}]{FG}"), folder];
    if !branch.is_empty() {
        let mut dirty = String::new();
        if staged > 0 {
            dirty.push_str(&format!("{GREEN}+{staged}{FG}"));
        }
        if modified > 0 {
            dirty.push_str(&format!("{YELLOW}~{modified}{FG}"));
        }
        left.push(if dirty.is_empty() {
            branch.to_string()
        } else {
            format!("{branch} {dirty}")
        });
    }
    if !effort.is_empty() {
        left.push(format!("{DIM}{effort}{FG}"));
    }
    let line1 = left.join(&format!("{DIM}  {FG}"));

    let pct = jf64(payload, &["context_window", "used_percentage"]);
    let used = ju64(payload, &["context_window", "total_input_tokens"]);
    let window = ju64(payload, &["context_window", "context_window_size"]);
    let color = pct_color(pct);
    let bar = progress_bar(pct, cfg.bar_width);
    let mut bits = vec![format!("{color}{bar}{FG} {color}{:.0}%{FG}", pct)];
    if window > 0 {
        bits[0].push_str(&format!(
            "{DIM} {}/{}{FG}",
            fmt_tokens(used),
            fmt_tokens(window)
        ));
    }
    let use_pct = payload.pointer("/usage/percent").and_then(|v| v.as_f64());
    if let Some(up) = use_pct {
        let extra = match jstr(payload, &["usage", "period"]) {
            "weekly" => "wk",
            "monthly" => "mo",
            other => other,
        };
        let reset = jstr(payload, &["usage", "reset"]);
        let mut label = format!("use {up:.0}%");
        if !extra.is_empty() {
            label.push(' ');
            label.push_str(extra);
        }
        if !reset.is_empty() {
            label.push(' ');
            label.push_str(reset);
        }
        bits.push(format!("{}{label}{FG}", pct_color(up)));
    }
    let dur_ms = ju64(payload, &["cost", "total_duration_ms"]);
    if dur_ms > 0 {
        bits.push(fmt_duration(dur_ms as f64 / 1000.0));
    }
    let added = ju64(payload, &["cost", "total_lines_added"]);
    let removed = ju64(payload, &["cost", "total_lines_removed"]);
    if added > 0 || removed > 0 {
        bits.push(format!("{GREEN}+{added}{FG}{DIM}/{RED}-{removed}{FG}"));
    }
    let line2 = bits.join(&format!("{DIM} │ {FG}"));

    if cfg.height <= 1 {
        let mut compact = format!("{CYAN}[{model}]{FG}{DIM} │ {FG}{color}{bar} {pct:.0}%{FG}");
        if let Some(up) = use_pct {
            compact.push_str(&format!(
                "{DIM} │ {FG}{}use {up:.0}%{FG}",
                pct_color(up)
            ));
        }
        if !branch.is_empty() {
            compact.push_str(&format!("{DIM} │ {FG}{branch}"));
        }
        return vec![pad_bar(&compact, cols, cfg.padding)];
    }
    vec![
        pad_bar(&line1, cols, cfg.padding),
        pad_bar(&line2, cols, cfg.padding),
    ]
}

fn run_user_command(cmd: &str, payload: &Value, cols: usize, timeout: f64) -> Option<Vec<String>> {
    let expanded = expand_cmd(cmd);
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(&expanded)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env("COLUMNS", cols.to_string())
        .env("LINES", MAX_HEIGHT.to_string())
        .spawn()
        .ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(payload.to_string().as_bytes());
    }
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                let out = child.wait_with_output().ok()?;
                let lines: Vec<String> = String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .map(|s| s.to_string())
                    .take(MAX_HEIGHT as usize)
                    .collect();
                if lines.is_empty() {
                    return None;
                }
                return Some(lines);
            }
            Ok(Some(_)) => return None,
            Ok(None) if start.elapsed().as_secs_f64() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(10)),
            Err(_) => return None,
        }
    }
}

pub fn render_status(payload: &Value, cols: usize, cfg: &Config) -> Vec<String> {
    if cfg.kind == Kind::Command && !cfg.command.is_empty() {
        if let Some(out) = run_user_command(&cfg.command, payload, cols, cfg.command_timeout) {
            return out;
        }
    }
    render_builtin(payload, cols, cfg)
}
