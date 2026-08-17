use crate::config::{dirs_home, grok_home, SHIM_MARK, UPSTREAM_NAME, VERSION};
use std::env;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub fn real_grok() -> Result<PathBuf, String> {
    if let Ok(bin) = env::var("GROK_BIN") {
        let p = PathBuf::from(bin);
        if p.is_file() {
            return Ok(p);
        }
    }
    let home = grok_home();
    let upstream = home.join("bin").join(UPSTREAM_NAME);
    if upstream.is_file() {
        return Ok(upstream);
    }
    let grok = home.join("bin").join("grok");
    if grok.is_file() && !is_shim(&grok) {
        return Ok(grok);
    }
    if let Ok(path) = env::var("PATH") {
        let self_exe = env::current_exe().ok();
        for dir in env::split_paths(&path) {
            let cand = dir.join("grok");
            if !cand.is_file() {
                continue;
            }
            if is_shim(&cand) {
                continue;
            }
            if let (Ok(a), Some(b)) = (cand.canonicalize(), self_exe.as_ref()) {
                if a == *b {
                    continue;
                }
            }
            return Ok(cand);
        }
    }
    Err("real grok not found (set GROK_BIN or install Grok Build)".into())
}

pub fn is_shim(path: &Path) -> bool {
    fs::read_to_string(path)
        .map(|s| s.contains(SHIM_MARK))
        .unwrap_or(false)
}

fn local_bin() -> PathBuf {
    dirs_home().join(".local").join("bin")
}

fn write_exec(path: &Path, contents: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms)?;
    Ok(())
}

fn tool_path() -> Result<PathBuf, String> {
    env::current_exe()
        .map_err(|e| e.to_string())?
        .canonicalize()
        .map_err(|e| e.to_string())
}

pub fn install() -> Result<(), String> {
    let tool = tool_path()?;
    let dest_tool = local_bin().join("grok-statusline");
    if dest_tool.canonicalize().ok().as_ref() != Some(&tool) {
        fs::create_dir_all(local_bin()).map_err(|e| e.to_string())?;
        let _ = fs::remove_file(&dest_tool);
        fs::copy(&tool, &dest_tool).map_err(|e| format!("copy binary: {e}"))?;
        let mut perms = fs::metadata(&dest_tool).map_err(|e| e.to_string())?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&dest_tool, perms).map_err(|e| e.to_string())?;
    }

    let grok_bin = grok_home().join("bin");
    fs::create_dir_all(&grok_bin).map_err(|e| e.to_string())?;
    let grok = grok_bin.join("grok");
    let upstream = grok_bin.join(UPSTREAM_NAME);

    if grok.exists() && !is_shim(&grok) {
        if upstream.exists() {
            return Err(format!(
                "{} exists and {} is not our shim; move it aside first",
                upstream.display(),
                grok.display()
            ));
        }
        fs::rename(&grok, &upstream).map_err(|e| format!("move grok aside: {e}"))?;
    } else if !upstream.exists() {
        // first install but grok already replaced, or grok missing
        if !grok.exists() {
            return Err("no grok binary in ~/.grok/bin to wrap".into());
        }
    }

    let shim = format!(
        "#!/bin/sh\n# {SHIM_MARK} {VERSION}\nexec \"{}\" wrap -- \"$@\"\n",
        dest_tool.display()
    );
    write_exec(&grok, &shim).map_err(|e| format!("write shim: {e}"))?;

    println!("installed {VERSION}");
    println!("  tool  {}", dest_tool.display());
    println!("  shim  {}", grok.display());
    println!("  real  {}", upstream.display());
    println!("launch `grok` as usual — no alias needed.");
    Ok(())
}

pub fn uninstall() -> Result<(), String> {
    let grok_bin = grok_home().join("bin");
    let grok = grok_bin.join("grok");
    let upstream = grok_bin.join(UPSTREAM_NAME);
    if grok.exists() && is_shim(&grok) {
        fs::remove_file(&grok).map_err(|e| e.to_string())?;
        if upstream.exists() {
            fs::rename(&upstream, &grok).map_err(|e| format!("restore grok: {e}"))?;
        }
        println!("removed shim; restored {}", grok.display());
    } else {
        println!("no shim installed at {}", grok.display());
    }
    Ok(())
}
