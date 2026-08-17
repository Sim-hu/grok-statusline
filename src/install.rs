use crate::config::{dirs_home, grok_home, SHIM_MARK, UPSTREAM_NAME, VERSION};
use std::env;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const PATH_BEGIN: &str = "# >>> grok-statusline >>>";
const PATH_END: &str = "# <<< grok-statusline <<<";

pub fn real_grok() -> Result<PathBuf, String> {
    if let Ok(bin) = env::var("GROK_BIN") {
        let p = PathBuf::from(bin);
        if p.is_file() {
            return Ok(p);
        }
    }
    let home = grok_home();
    let official = home.join("bin").join("grok");
    // Prefer the official binary so Grok updates (which rewrite this path) are picked up.
    if official.is_file() && !is_shim(&official) {
        return Ok(official);
    }
    let upstream = home.join("bin").join(UPSTREAM_NAME);
    if upstream.is_file() {
        return Ok(upstream);
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
    // Unlink first so we do not follow a symlink onto the running grok binary
    // (that returns ETXTBSY).
    let _ = fs::remove_file(path);
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

fn copy_tool(dest: &Path) -> Result<(), String> {
    let tool = tool_path()?;
    if dest.canonicalize().ok().as_ref() == Some(&tool) {
        return Ok(());
    }
    fs::create_dir_all(local_bin()).map_err(|e| e.to_string())?;
    let _ = fs::remove_file(dest);
    fs::copy(&tool, dest).map_err(|e| format!("copy binary: {e}"))?;
    let mut perms = fs::metadata(dest).map_err(|e| e.to_string())?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(dest, perms).map_err(|e| e.to_string())?;
    Ok(())
}

/// Old installs replaced ~/.grok/bin/grok. Put the official binary back so
/// Grok's updater can keep rewriting that path.
fn restore_official_grok() -> Result<Option<PathBuf>, String> {
    let grok_bin = grok_home().join("bin");
    let official = grok_bin.join("grok");
    let upstream = grok_bin.join(UPSTREAM_NAME);
    if official.exists() && is_shim(&official) {
        fs::remove_file(&official).map_err(|e| e.to_string())?;
        if upstream.exists() {
            fs::rename(&upstream, &official).map_err(|e| format!("restore grok: {e}"))?;
        }
    }
    if official.is_file() && !is_shim(&official) {
        let _ = fs::remove_file(&upstream);
        return Ok(Some(official));
    }
    if official.exists() {
        return Err(format!(
            "{} is still a shim and {} is missing; reinstall Grok Build first",
            official.display(),
            upstream.display()
        ));
    }
    Ok(None)
}

fn shim_script() -> String {
    format!(
        "#!/bin/sh\n# {SHIM_MARK} {VERSION}\nexec \"$HOME/.local/bin/grok-statusline\" wrap -- \"$@\"\n"
    )
}

fn path_snippet_sh() -> String {
    format!(
        "{PATH_BEGIN}\n# After the grok installer block. Official grok stays in ~/.grok/bin and can update.\nexport PATH=\"$HOME/.local/bin:$PATH\"\n{PATH_END}\n"
    )
}

fn path_snippet_fish() -> String {
    format!(
        "{PATH_BEGIN}\n# After the grok installer block. Official grok stays in ~/.grok/bin and can update.\nfish_add_path -p $HOME/.local/bin\n{PATH_END}\n"
    )
}

fn strip_block(text: &str) -> String {
    let mut out = String::new();
    let mut skipping = false;
    for line in text.lines() {
        let t = line.trim();
        if t == PATH_BEGIN {
            skipping = true;
            continue;
        }
        if skipping {
            if t == PATH_END {
                skipping = false;
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn upsert_block(path: &Path, snippet: &str) -> Result<(), String> {
    let original = fs::read_to_string(path).unwrap_or_default();
    let mut text = strip_block(&original);
    while text.ends_with("\n\n") {
        text.pop();
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    if !text.is_empty() {
        text.push('\n');
    }
    text.push_str(snippet);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    if text != original {
        fs::write(path, text).map_err(|e| format!("write {}: {e}", path.display()))?;
    }
    Ok(())
}

fn remove_block(path: &Path) -> Result<(), String> {
    let Ok(original) = fs::read_to_string(path) else {
        return Ok(());
    };
    let mut text = strip_block(&original);
    while text.ends_with("\n\n") {
        text.pop();
    }
    if !text.ends_with('\n') && !text.is_empty() {
        text.push('\n');
    }
    if text != original {
        fs::write(path, text).map_err(|e| format!("write {}: {e}", path.display()))?;
    }
    Ok(())
}

fn ensure_path_hooks() -> Result<Vec<PathBuf>, String> {
    let home = dirs_home();
    let mut touched = Vec::new();
    let bashrc = home.join(".bashrc");
    upsert_block(&bashrc, &path_snippet_sh())?;
    touched.push(bashrc);
    for name in [".zshrc", ".zshenv"] {
        let p = home.join(name);
        if p.exists() {
            upsert_block(&p, &path_snippet_sh())?;
            touched.push(p);
        }
    }
    let fish = home.join(".config").join("fish").join("config.fish");
    if fish.exists() {
        upsert_block(&fish, &path_snippet_fish())?;
        touched.push(fish);
    }
    Ok(touched)
}

fn remove_path_hooks() -> Result<(), String> {
    let home = dirs_home();
    for name in [".bashrc", ".zshrc", ".zshenv"] {
        remove_block(&home.join(name))?;
    }
    remove_block(&home.join(".config").join("fish").join("config.fish"))?;
    Ok(())
}

pub fn install() -> Result<(), String> {
    let dest_tool = local_bin().join("grok-statusline");
    copy_tool(&dest_tool)?;

    let official = restore_official_grok()?;
    let dest_grok = local_bin().join("grok");
    write_exec(&dest_grok, &shim_script()).map_err(|e| format!("write wrapper: {e}"))?;
    let hooks = ensure_path_hooks()?;

    println!("installed {VERSION}");
    println!("  tool     {}", dest_tool.display());
    println!("  wrapper  {}", dest_grok.display());
    match official {
        Some(p) => println!("  grok     {}  (updater owns this)", p.display()),
        None => println!("  grok     (not found under ~/.grok/bin — wrapper will search PATH)"),
    }
    for h in hooks {
        println!("  path     {}", h.display());
    }
    println!("Grok can update ~/.grok/bin/grok in place. No reinstall needed.");
    println!("This shell:  export PATH=\"$HOME/.local/bin:$PATH\" && hash -r");
    Ok(())
}

pub fn uninstall() -> Result<(), String> {
    let dest_grok = local_bin().join("grok");
    if dest_grok.exists() && is_shim(&dest_grok) {
        fs::remove_file(&dest_grok).map_err(|e| e.to_string())?;
        println!("removed {}", dest_grok.display());
    }
    let _ = restore_official_grok();
    remove_path_hooks()?;
    println!("removed PATH hooks; `grok` is the official binary again");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_keeps_surrounding() {
        let src = "export PATH=/usr/bin\n# >>> grok-statusline >>>\nold\n# <<< grok-statusline <<<\n# done\n";
        assert_eq!(strip_block(src), "export PATH=/usr/bin\n# done\n");
    }

    #[test]
    fn strip_noop() {
        assert_eq!(strip_block("hello\n"), "hello\n");
    }

    #[test]
    fn is_shim_detects_marker() {
        let dir = std::env::temp_dir().join(format!("gsl-shim-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("grok");
        fs::write(&p, shim_script()).unwrap();
        assert!(is_shim(&p));
        fs::write(&p, "#!/bin/sh\nexec /opt/fivem/.grok/bin/grok-upstream\n").unwrap();
        assert!(!is_shim(&p));
        let _ = fs::remove_dir_all(&dir);
    }
}
