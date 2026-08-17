use crate::config::Config;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::Instant;

#[derive(Clone, Debug, Default)]
pub struct GitInfo {
    pub branch: String,
    pub staged: u32,
    pub modified: u32,
    pub untracked: u32,
    pub ahead: u32,
    pub behind: u32,
    pub origin_url: String,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct RepoId {
    pub host: String,
    pub owner: String,
    pub name: String,
}

struct CacheEntry {
    at: Instant,
    info: GitInfo,
}

static CACHE: Mutex<Vec<(PathBuf, CacheEntry)>> = Mutex::new(Vec::new());

pub fn git_info(cwd: &Path, cfg: &Config) -> GitInfo {
    let now = Instant::now();
    if let Ok(cache) = CACHE.lock() {
        if let Some((_, hit)) = cache.iter().find(|(p, _)| p == cwd) {
            if now.duration_since(hit.at).as_secs_f64() < cfg.git_cache_seconds {
                return hit.info.clone();
            }
        }
    }
    let info = probe(cwd, cfg.git_untracked);
    if let Ok(mut cache) = CACHE.lock() {
        cache.retain(|(p, _)| p != cwd);
        cache.push((
            cwd.to_path_buf(),
            CacheEntry {
                at: now,
                info: info.clone(),
            },
        ));
    }
    info
}

fn probe(cwd: &Path, untracked: bool) -> GitInfo {
    let mut args = vec![
        "-C",
        cwd.to_str().unwrap_or("."),
        "status",
        "--porcelain=v1",
        "-b",
    ];
    if !untracked {
        args.push("--untracked-files=no");
    }
    let out = Command::new("git")
        .args(&args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output();
    let Ok(out) = out else {
        return GitInfo::default();
    };
    if !out.status.success() {
        return GitInfo::default();
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut info = GitInfo::default();
    let mut lines = text.lines();
    if let Some(first) = lines.next() {
        if let Some(head) = first.strip_prefix("## ") {
            let name = head.split("...").next().unwrap_or(head).trim();
            if name != "HEAD (no branch)" {
                info.branch = name.to_string();
            }
            if let Some(n) = capture_after(head, "ahead ") {
                info.ahead = n;
            }
            if let Some(n) = capture_after(head, "behind ") {
                info.behind = n;
            }
        } else {
            parse_xy(&mut info, first);
        }
    }
    for line in lines {
        parse_xy(&mut info, line);
    }
    if let Ok(rem) = Command::new("git")
        .args(["-C", cwd.to_str().unwrap_or("."), "remote", "get-url", "origin"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
    {
        if rem.status.success() {
            info.origin_url = String::from_utf8_lossy(&rem.stdout).trim().to_string();
        }
    }
    info
}

fn parse_xy(info: &mut GitInfo, line: &str) {
    let b = line.as_bytes();
    if b.len() < 2 {
        return;
    }
    let x = b[0] as char;
    let y = b[1] as char;
    if x == '?' && y == '?' {
        info.untracked += 1;
        return;
    }
    if x != ' ' && x != '?' {
        info.staged += 1;
    }
    if y != ' ' && y != '?' {
        info.modified += 1;
    }
}

fn capture_after(s: &str, key: &str) -> Option<u32> {
    let i = s.find(key)?;
    s[i + key.len()..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .ok()
}

pub fn parse_origin(url: &str) -> Option<RepoId> {
    let raw = url.trim();
    if raw.is_empty() {
        return None;
    }
    let (host, path) = if let Some(rest) = raw.strip_prefix("git@") {
        let (host, path) = rest.split_once(':')?;
        (host.to_string(), path.to_string())
    } else {
        let rest = raw
            .strip_prefix("https://")
            .or_else(|| raw.strip_prefix("http://"))
            .or_else(|| raw.strip_prefix("ssh://"))
            .unwrap_or(raw);
        let rest = rest.strip_prefix("git@").unwrap_or(rest);
        let (host, path) = rest.split_once('/')?;
        (host.to_string(), path.to_string())
    };
    let path = path.trim_end_matches(".git");
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    if host.is_empty() || parts.len() < 2 {
        return None;
    }
    Some(RepoId {
        host,
        owner: parts[parts.len() - 2].to_string(),
        name: parts[parts.len() - 1].to_string(),
    })
}
