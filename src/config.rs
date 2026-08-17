use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const DEFAULT_WINDOW: u64 = 500_000;
pub const MAX_HEIGHT: u16 = 3;
pub const SHIM_MARK: &str = "grok-statusline-shim";
pub const UPSTREAM_NAME: &str = "grok-upstream";

#[derive(Clone, Debug)]
pub struct Config {
    pub kind: Kind,
    pub command: String,
    pub padding: usize,
    pub refresh_interval: f64,
    pub height: u16,
    pub bar_width: usize,
    pub usage_enabled: bool,
    pub git_enabled: bool,
    pub git_cache_seconds: f64,
    pub git_untracked: bool,
    pub command_timeout: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Builtin,
    Command,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            kind: Kind::Builtin,
            command: String::new(),
            padding: 0,
            refresh_interval: 1.0,
            height: 2,
            bar_width: 10,
            usage_enabled: true,
            git_enabled: true,
            git_cache_seconds: 5.0,
            git_untracked: false,
            command_timeout: 0.2,
        }
    }
}

#[derive(Default, Deserialize)]
struct FileCfg {
    #[serde(rename = "statusLine", default)]
    status_line: StatusLineCfg,
    #[serde(default)]
    usage: UsageCfg,
    #[serde(default)]
    git: GitCfg,
}

#[derive(Default, Deserialize)]
struct StatusLineCfg {
    #[serde(rename = "type")]
    kind: Option<String>,
    command: Option<String>,
    padding: Option<u32>,
    #[serde(rename = "refreshInterval")]
    refresh_interval: Option<f64>,
    height: Option<u16>,
    #[serde(rename = "barWidth")]
    bar_width: Option<u32>,
    #[serde(rename = "commandTimeout")]
    command_timeout: Option<f64>,
}

#[derive(Default, Deserialize)]
struct UsageCfg {
    enabled: Option<bool>,
}

#[derive(Default, Deserialize)]
struct GitCfg {
    enabled: Option<bool>,
    #[serde(rename = "cacheSeconds")]
    cache_seconds: Option<f64>,
    untracked: Option<bool>,
}

pub fn grok_home() -> PathBuf {
    if let Ok(raw) = std::env::var("GROK_HOME") {
        return PathBuf::from(raw);
    }
    dirs_home().join(".grok")
}

pub fn dirs_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

pub fn load_config(home: &Path, cwd: &Path) -> Config {
    let mut cfg = Config::default();
    for path in [
        home.join("grok-statusline.json"),
        cwd.join(".grok").join("grok-statusline.json"),
    ] {
        if let Ok(text) = fs::read_to_string(&path) {
            if let Ok(file) = serde_json::from_str::<FileCfg>(&text) {
                apply(&mut cfg, file);
            }
        }
    }
    cfg
}

fn apply(cfg: &mut Config, file: FileCfg) {
    if let Some(kind) = file.status_line.kind {
        cfg.kind = match kind.to_ascii_lowercase().as_str() {
            "command" => Kind::Command,
            _ => Kind::Builtin,
        };
    }
    if let Some(cmd) = file.status_line.command {
        cfg.command = cmd;
    }
    if let Some(p) = file.status_line.padding {
        cfg.padding = p as usize;
    }
    if let Some(r) = file.status_line.refresh_interval {
        cfg.refresh_interval = r.max(0.3);
    }
    if let Some(h) = file.status_line.height {
        cfg.height = h.clamp(1, MAX_HEIGHT);
    }
    if let Some(w) = file.status_line.bar_width {
        cfg.bar_width = (w as usize).clamp(4, 20);
    }
    if let Some(t) = file.status_line.command_timeout {
        cfg.command_timeout = t.max(0.05);
    }
    if let Some(v) = file.usage.enabled {
        cfg.usage_enabled = v;
    }
    if let Some(v) = file.git.enabled {
        cfg.git_enabled = v;
    }
    if let Some(v) = file.git.cache_seconds {
        cfg.git_cache_seconds = v.max(1.0);
    }
    if let Some(v) = file.git.untracked {
        cfg.git_untracked = v;
    }
}

pub fn expand_cmd(cmd: &str) -> String {
    let home = dirs_home().to_string_lossy().into_owned();
    cmd.replace('~', &home)
}
