use crate::config::{Config, DEFAULT_WINDOW, VERSION};
use crate::git::{git_info, parse_origin};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Default)]
pub struct Usage {
    pub percent: f64,
    pub period: String,
    pub period_end: Option<String>,
}

struct FileHit {
    mtime_ns: u128,
    size: u64,
    value: Value,
}

static FILES: Mutex<Vec<(PathBuf, FileHit)>> = Mutex::new(Vec::new());

pub fn read_json(path: &Path) -> Option<Value> {
    let meta = fs::metadata(path).ok()?;
    let mtime_ns = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let size = meta.len();
    if let Ok(cache) = FILES.lock() {
        if let Some((_, hit)) = cache.iter().find(|(p, _)| p == path) {
            if hit.mtime_ns == mtime_ns && hit.size == size {
                return Some(hit.value.clone());
            }
        }
    }
    let text = fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    if let Ok(mut cache) = FILES.lock() {
        cache.retain(|(p, _)| p != path);
        cache.push((
            path.to_path_buf(),
            FileHit {
                mtime_ns,
                size,
                value: value.clone(),
            },
        ));
    }
    Some(value)
}

fn encode_cwd(cwd: &Path) -> String {
    let mut parts = Vec::new();
    for c in cwd.components() {
        if let std::path::Component::Normal(s) = c {
            parts.push(percent_encode(&s.to_string_lossy()));
        }
    }
    format!("%2F{}", parts.join("%2F"))
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn score_session(dir: &Path) -> (bool, u64) {
    let has = dir.join("signals.json").is_file() || dir.join("updates.jsonl").is_file();
    let mtime = ["signals.json", "updates.jsonl", "summary.json"]
        .iter()
        .find_map(|n| fs::metadata(dir.join(n)).ok()?.modified().ok())
        .or_else(|| fs::metadata(dir).ok()?.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    (has, mtime)
}

fn find_session(home: &Path, session_id: &str) -> Option<PathBuf> {
    let root = home.join("sessions");
    for ent in fs::read_dir(root).ok()? {
        let ent = ent.ok()?;
        let cand = ent.path().join(session_id);
        if cand.is_dir() {
            return Some(cand);
        }
    }
    None
}

pub fn pick_session_dir(home: &Path, cwd: &Path) -> Option<PathBuf> {
    let cwd_res = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let mut best: Option<(bool, u64, PathBuf)> = None;
    if let Some(Value::Array(active)) = read_json(&home.join("active_sessions.json")) {
        for entry in active {
            let sid = entry.get("session_id")?.as_str()?;
            if let Some(ecwd) = entry.get("cwd").and_then(|v| v.as_str()) {
                let other = PathBuf::from(ecwd);
                let other = other.canonicalize().unwrap_or(other);
                if other != cwd_res {
                    continue;
                }
            }
            let encoded = encode_cwd(entry.get("cwd").and_then(|v| v.as_str()).map(Path::new).unwrap_or(cwd));
            let mut dir = home.join("sessions").join(encoded).join(sid);
            if !dir.is_dir() {
                match find_session(home, sid) {
                    Some(found) => dir = found,
                    None => continue,
                }
            }
            let (has, mtime) = score_session(&dir);
            let better = match &best {
                None => true,
                Some((b_has, b_m, _)) => (has, mtime) > (*b_has, *b_m),
            };
            if better {
                best = Some((has, mtime, dir));
            }
        }
    }
    if let Some((_, _, dir)) = best {
        return Some(dir);
    }
    let encoded = home.join("sessions").join(encode_cwd(cwd));
    let mut kids = Vec::new();
    if let Ok(rd) = fs::read_dir(&encoded) {
        for ent in rd.flatten() {
            let p = ent.path();
            if p.is_dir() {
                let (has, mtime) = score_session(&p);
                kids.push((has, mtime, p));
            }
        }
    }
    kids.sort();
    kids.pop().map(|(_, _, p)| p)
}

fn tokens_from_updates(path: &Path) -> u64 {
    let Ok(meta) = fs::metadata(path) else {
        return 0;
    };
    let size = meta.len();
    let take = size.min(64 * 1024);
    let Ok(mut f) = fs::File::open(path) else {
        return 0;
    };
    use std::io::{Read, Seek, SeekFrom};
    if f.seek(SeekFrom::Start(size.saturating_sub(take))).is_err() {
        return 0;
    }
    let mut buf = String::new();
    if f.read_to_string(&mut buf).is_err() {
        return 0;
    }
    let mut best = 0u64;
    for line in buf.lines() {
        if !line.contains("totalTokens") {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            if let Some(n) = v
                .pointer("/params/_meta/totalTokens")
                .and_then(|x| x.as_u64().or_else(|| x.as_f64().map(|f| f as u64)))
            {
                best = best.max(n);
            }
        }
    }
    best
}

fn as_u64(v: Option<&Value>) -> u64 {
    v.and_then(|x| x.as_u64().or_else(|| x.as_f64().map(|f| f as u64)))
        .unwrap_or(0)
}

fn as_f64(v: Option<&Value>) -> f64 {
    v.and_then(|x| x.as_f64().or_else(|| x.as_u64().map(|n| n as f64)))
        .unwrap_or(0.0)
}

fn parse_iso(raw: &str) -> Option<f64> {
    let raw = raw.replace('Z', "+00:00");
    let (date, rest) = raw.split_once('T')?;
    let mut ds = date.split('-');
    let y: i32 = ds.next()?.parse().ok()?;
    let mo: u32 = ds.next()?.parse().ok()?;
    let d: u32 = ds.next()?.parse().ok()?;
    let rest = rest.split('+').next()?.split('-').next()?;
    let mut ts = rest.split(':');
    let h: u32 = ts.next()?.parse().ok()?;
    let mi: u32 = ts.next()?.parse().ok()?;
    let s: f64 = ts.next()?.parse().ok()?;
    use std::time::Duration;
    // UTC approx via time crate-less days from civil
    let days = days_from_civil(y, mo, d)?;
    let secs = days as f64 * 86400.0 + h as f64 * 3600.0 + mi as f64 * 60.0 + s;
    let _ = Duration::from_secs(1);
    Some(secs)
}

fn days_from_civil(y: i32, m: u32, d: u32) -> Option<i64> {
    if !(1..=12).contains(&m) || d == 0 || d > 31 {
        return None;
    }
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let mp = if m > 2 { m - 3 } else { m + 9 } as u64;
    let doy = (153 * mp + 2) / 5 + d as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era as i64 * 146097 + doe as i64 - 719468)
}

pub fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub fn fmt_reset(period_end: Option<&str>) -> String {
    let Some(raw) = period_end else {
        return String::new();
    };
    let Some(end) = parse_iso(raw) else {
        return String::new();
    };
    let ms = end - now_secs();
    if ms <= 0.0 {
        return "soon".into();
    }
    let hours = (ms / 3600.0) as i64;
    if hours < 48 {
        format!("{}h", hours.max(1))
    } else {
        format!("{}d", hours / 24)
    }
}

fn display_model(id: &str) -> String {
    if let Some(rest) = id.strip_prefix("grok-") {
        format!("Grok {rest}")
    } else {
        id.to_string()
    }
}

pub fn build_payload(home: &Path, cwd: &Path, cfg: &Config, usage: Option<&Usage>) -> Value {
    let session_dir = pick_session_dir(home, cwd);
    let signals = session_dir
        .as_ref()
        .and_then(|d| read_json(&d.join("signals.json")))
        .unwrap_or(json!({}));
    let summary = session_dir
        .as_ref()
        .and_then(|d| read_json(&d.join("summary.json")))
        .unwrap_or(json!({}));

    let mut model_id = summary
        .get("current_model_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if model_id.is_empty() {
        model_id = signals
            .get("primaryModelId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
    }

    let mut ctx_used = as_u64(signals.get("contextTokensUsed"));
    let mut ctx_window = as_u64(signals.get("contextWindowTokens"));
    let mut ctx_pct = as_f64(signals.get("contextWindowUsage"));
    if ctx_used == 0 {
        if let Some(dir) = &session_dir {
            let est = tokens_from_updates(&dir.join("updates.jsonl"));
            if est > 0 {
                ctx_used = est;
                if ctx_window == 0 {
                    ctx_window = DEFAULT_WINDOW;
                }
            }
        }
    }
    if ctx_window == 0 && ctx_used > 0 {
        ctx_window = DEFAULT_WINDOW;
    }
    if ctx_window > 0 && ctx_pct <= 0.0 && ctx_used > 0 {
        ctx_pct = 100.0 * ctx_used as f64 / ctx_window as f64;
    }

    let session_id = summary
        .pointer("/info/id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            session_dir
                .as_ref()
                .and_then(|d| d.file_name())
                .map(|s| s.to_string_lossy().into_owned())
        })
        .unwrap_or_default();

    let duration_s = summary
        .get("created_at")
        .and_then(|v| v.as_str())
        .and_then(parse_iso)
        .map(|start| (now_secs() - start).max(0.0))
        .unwrap_or_else(|| as_f64(signals.get("sessionDurationSeconds")));

    let added = as_u64(signals.get("agentLinesAdded"));
    let removed = as_u64(signals.get("agentLinesRemoved"));

    let g = if cfg.git_enabled {
        git_info(cwd, cfg)
    } else {
        crate::git::GitInfo::default()
    };
    let repo = parse_origin(&g.origin_url);

    let mut payload = json!({
        "cwd": cwd.to_string_lossy(),
        "session_id": session_id,
        "transcript_path": session_dir.as_ref().map(|d| d.join("updates.jsonl").display().to_string()).unwrap_or_default(),
        "version": VERSION,
        "model": { "id": model_id, "display_name": display_model(&model_id) },
        "workspace": {
            "current_dir": cwd.to_string_lossy(),
            "project_dir": cwd.to_string_lossy(),
        },
        "cost": {
            "total_duration_ms": (duration_s * 1000.0) as u64,
            "total_lines_added": added,
            "total_lines_removed": removed,
        },
        "context_window": {
            "total_input_tokens": ctx_used,
            "total_output_tokens": 0,
            "context_window_size": ctx_window,
            "used_percentage": ctx_pct,
            "remaining_percentage": (100.0 - ctx_pct).max(0.0),
        },
        "exceeds_200k_tokens": ctx_used > 200_000,
        "git": {
            "branch": g.branch,
            "staged": g.staged,
            "modified": g.modified,
            "untracked": g.untracked,
            "ahead": g.ahead,
            "behind": g.behind,
        },
    });

    if let Some(title) = summary
        .get("generated_title")
        .or_else(|| summary.get("session_summary"))
        .and_then(|v| v.as_str())
    {
        payload["session_name"] = json!(title);
    }
    if let Some(effort) = summary.get("reasoning_effort").and_then(|v| v.as_str()) {
        payload["effort"] = json!({ "level": effort });
    }
    if let Some(agent) = summary.get("agent_name").and_then(|v| v.as_str()) {
        payload["agent"] = json!({ "name": agent });
    }
    if let Some(repo) = repo {
        payload["workspace"]["repo"] = json!(repo);
    }
    let mut extra = HashMap::new();
    if let Some(n) = signals.get("turnCount").and_then(|v| v.as_u64()) {
        extra.insert("turn_count", n);
    }
    if let Some(n) = signals.get("toolCallCount").and_then(|v| v.as_u64()) {
        extra.insert("tool_call_count", n);
    }
    if let Some(n) = signals.get("compactionCount").and_then(|v| v.as_u64()) {
        extra.insert("compaction_count", n);
    }
    if !extra.is_empty() {
        payload["grok"] = json!(extra);
    }
    if let Some(u) = usage {
        let key = if u.period == "weekly" {
            "seven_day"
        } else {
            "five_hour"
        };
        let mut window = json!({ "used_percentage": u.percent });
        if let Some(end) = u.period_end.as_deref().and_then(parse_iso) {
            window["resets_at"] = json!(end as u64);
        }
        payload["rate_limits"] = json!({ key: window });
        payload["usage"] = json!({
            "percent": u.percent,
            "period": u.period,
            "reset": fmt_reset(u.period_end.as_deref()),
        });
    }
    payload
}

pub fn fetch_usage(home: &Path) -> Option<Usage> {
    let cache_path = home.join("grok-statusline").join("billing-cache.json");
    if let Ok(text) = fs::read_to_string(&cache_path) {
        if let Ok(cached) = serde_json::from_str::<Value>(&text) {
            let age = now_secs() - cached.get("fetched_at").and_then(|v| v.as_f64()).unwrap_or(0.0);
            if (0.0..60.0).contains(&age) {
                if let Some(u) = usage_from_value(cached.get("data")) {
                    return Some(u);
                }
            }
        }
    }

    let token = auth_token(home)?;
    let credits = http_json(
        "https://cli-chat-proxy.grok.com/v1/billing?format=credits",
        &token,
    );
    let monthly = http_json("https://cli-chat-proxy.grok.com/v1/billing", &token);
    let mut data = Value::Object(serde_json::Map::new());

    if let Some(credits) = credits {
        let cfg = credits.get("config").cloned().unwrap_or(credits);
        let mut pct = cfg.get("creditUsagePercent").and_then(|v| v.as_f64());
        if pct.is_none() {
            pct = cfg
                .get("productUsage")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|p| p.get("usagePercent"))
                .and_then(|v| v.as_f64());
        }
        if let Some(p) = pct {
            data["percent"] = json!(p.clamp(0.0, 100.0));
            data["periodType"] = json!("weekly");
        }
        if let Some(end) = cfg.pointer("/currentPeriod/end").and_then(|v| v.as_str()) {
            data["periodEnd"] = json!(end);
            data["weekEnd"] = json!(end);
        }
    }

    if data.get("percent").is_none() {
        if let Some(monthly) = monthly {
            let cfg = monthly.get("config").cloned().unwrap_or(monthly);
            let used = num_val(cfg.get("used"));
            let limit = num_val(cfg.get("monthlyLimit"));
            if let (Some(used), Some(limit)) = (used, limit) {
                if limit > 0.0 {
                    data["percent"] = json!((100.0 * used / limit).clamp(0.0, 100.0));
                    data["periodType"] = json!("monthly");
                    if let Some(end) = cfg.get("billingPeriodEnd").and_then(|v| v.as_str()) {
                        data["periodEnd"] = json!(end);
                    }
                }
            }
        }
    }

    if data.get("percent").is_none() {
        return None;
    }

    let _ = fs::create_dir_all(cache_path.parent().unwrap());
    let _ = fs::write(
        &cache_path,
        serde_json::to_string_pretty(&json!({ "fetched_at": now_secs(), "data": data })).unwrap_or_default()
            + "\n",
    );
    usage_from_value(Some(&data))
}

fn usage_from_value(v: Option<&Value>) -> Option<Usage> {
    let v = v?;
    Some(Usage {
        percent: v.get("percent")?.as_f64()?,
        period: v
            .get("periodType")
            .and_then(|x| x.as_str())
            .unwrap_or("weekly")
            .to_string(),
        period_end: v
            .get("periodEnd")
            .or_else(|| v.get("weekEnd"))
            .and_then(|x| x.as_str())
            .map(|s| s.to_string()),
    })
}

fn num_val(v: Option<&Value>) -> Option<f64> {
    let v = v?;
    v.as_f64()
        .or_else(|| v.as_u64().map(|n| n as f64))
        .or_else(|| v.get("val").and_then(|x| x.as_f64()))
}

fn auth_token(home: &Path) -> Option<String> {
    let raw = read_json(&home.join("auth.json"))?;
    let obj = raw.as_object()?;
    for entry in obj.values() {
        for key in ["key", "access_token"] {
            if let Some(s) = entry.get(key).and_then(|v| v.as_str()) {
                if s.len() > 10 {
                    return Some(s.to_string());
                }
            }
        }
    }
    None
}

fn http_json(url: &str, token: &str) -> Option<Value> {
    // curl avoids linking rustls/ring (this host's cc cannot build ring).
    let out = std::process::Command::new("curl")
        .args([
            "-sS",
            "--max-time",
            "8",
            "-H",
            &format!("Authorization: Bearer {token}"),
            "-H",
            "Accept: application/json",
            "-A",
            &format!("grok-statusline/{VERSION}"),
            url,
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    serde_json::from_slice(&out.stdout).ok()
}
