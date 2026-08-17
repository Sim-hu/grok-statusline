mod config;
mod git;
mod install;
mod render;
mod session;
mod wrap;

use config::{grok_home, load_config, Config, VERSION};
use session::{build_payload, fetch_usage};
use std::env;
use std::io::Write;
use std::path::PathBuf;

fn invoked_as_grok() -> bool {
    env::args_os()
        .next()
        .and_then(|a| {
            PathBuf::from(a)
                .file_name()
                .map(|s| s.to_string_lossy() == "grok")
        })
        .unwrap_or(false)
}

fn print_help() {
    print!(
        "\
grok-statusline {VERSION} — Claude-style bottom statusline for Grok

Usage:
  grok                         after install, this is enough
  grok-statusline install      put a wrapper on PATH; leave official grok alone
  grok-statusline uninstall    remove the wrapper
  grok-statusline wrap [--] [grok args...]
  grok-statusline once [--no-usage] [--height N]
  grok-statusline dump-json [--no-usage]
  grok-statusline --help
"
    );
}

struct Flags {
    no_usage: bool,
    height: Option<u16>,
    rest: Vec<String>,
}

fn parse_flags(args: &[String]) -> Flags {
    let mut no_usage = false;
    let mut height = None;
    let mut rest = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--no-usage" => no_usage = true,
            "--height" => {
                i += 1;
                if let Some(v) = args.get(i).and_then(|s| s.parse().ok()) {
                    height = Some(v);
                }
            }
            s if s.starts_with("--height=") => {
                height = s.split_once('=').and_then(|(_, v)| v.parse().ok());
            }
            "--" => {
                rest.extend(args[i + 1..].iter().cloned());
                break;
            }
            other => rest.push(other.to_string()),
        }
        i += 1;
    }
    Flags {
        no_usage,
        height,
        rest,
    }
}

fn apply_flags(cfg: &mut Config, flags: &Flags) {
    if flags.no_usage || env::var("GROK_SL_NO_USAGE").ok().as_deref() == Some("1") {
        cfg.usage_enabled = false;
    }
    if let Some(h) = flags.height {
        cfg.height = h.clamp(1, 3);
    }
}

fn cmd_once(dump_json: bool, flags: Flags) -> i32 {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let home = grok_home();
    let mut cfg = load_config(&home, &cwd);
    apply_flags(&mut cfg, &flags);
    let usage = if cfg.usage_enabled {
        fetch_usage(&home)
    } else {
        None
    };
    let payload = build_payload(&home, &cwd, &cfg, usage.as_ref());
    if dump_json {
        println!("{}", serde_json::to_string_pretty(&payload).unwrap_or_default());
        return 0;
    }
    let cols = terminal_cols();
    let lines = render::render_status(&payload, cols, &cfg);
    let mut out = std::io::stdout();
    for line in lines {
        let _ = writeln!(out, "{line}");
    }
    0
}

fn terminal_cols() -> usize {
    let mut ws = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let r = unsafe { libc::ioctl(1, libc::TIOCGWINSZ, &mut ws) };
    if r == 0 && ws.ws_col > 0 {
        ws.ws_col as usize
    } else {
        80
    }
}

fn main() {
    let mut args: Vec<String> = env::args().skip(1).collect();
    if invoked_as_grok() {
        let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let mut cfg = load_config(&grok_home(), &cwd);
        let flags = parse_flags(&args);
        apply_flags(&mut cfg, &flags);
        std::process::exit(wrap::run_wrap(&flags.rest, cfg));
    }

    let cmd = args.first().map(|s| s.as_str()).unwrap_or("wrap");
    let code = match cmd {
        "-h" | "--help" | "help" => {
            print_help();
            0
        }
        "-V" | "--version" | "version" => {
            println!("{VERSION}");
            0
        }
        "install" => match install::install() {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("grok-statusline: {e}");
                1
            }
        },
        "uninstall" => match install::uninstall() {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("grok-statusline: {e}");
                1
            }
        },
        "once" | "--once" => {
            args.remove(0);
            cmd_once(false, parse_flags(&args))
        }
        "dump-json" | "--dump-json" => {
            args.remove(0);
            cmd_once(true, parse_flags(&args))
        }
        "wrap" => {
            args.remove(0);
            let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let mut cfg = load_config(&grok_home(), &cwd);
            let flags = parse_flags(&args);
            apply_flags(&mut cfg, &flags);
            wrap::run_wrap(&flags.rest, cfg)
        }
        _ => {
            let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let mut cfg = load_config(&grok_home(), &cwd);
            let flags = parse_flags(&args);
            apply_flags(&mut cfg, &flags);
            wrap::run_wrap(&flags.rest, cfg)
        }
    };
    std::process::exit(code);
}
