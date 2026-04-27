use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

use super::consts::{EVENT, HOOK_COMMAND, MATCHER};

pub fn global_path() -> Option<PathBuf> {
    dirs_home().map(|h| h.join(".claude").join("settings.json"))
}

pub fn local_path() -> PathBuf {
    PathBuf::from(".claude").join("settings.json")
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

pub fn install(path: &Path) -> Result<InstallOutcome> {
    let value = read_or_empty(path)?;
    let mut value = ensure_object(value);
    let already = has_hook(&value);
    if already {
        return Ok(InstallOutcome::AlreadyInstalled);
    }
    insert_hook(&mut value);
    write_with_backup(path, &value)?;
    Ok(InstallOutcome::Installed)
}

pub fn uninstall(path: &Path) -> Result<UninstallOutcome> {
    if !path.exists() {
        return Ok(UninstallOutcome::NoFile);
    }
    let value = read_or_empty(path)?;
    let mut value = ensure_object(value);
    let removed = remove_hook(&mut value);
    if !removed {
        return Ok(UninstallOutcome::NotPresent);
    }
    write_with_backup(path, &value)?;
    Ok(UninstallOutcome::Removed)
}

pub enum InstallOutcome {
    Installed,
    AlreadyInstalled,
}

pub enum UninstallOutcome {
    Removed,
    NotPresent,
    NoFile,
}

fn read_or_empty(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(Value::Object(Default::default()));
    }
    let content = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    if content.trim().is_empty() {
        return Ok(Value::Object(Default::default()));
    }
    let value: Value = serde_json::from_str(&content)
        .with_context(|| format!("parse {} as JSON", path.display()))?;
    Ok(value)
}

fn ensure_object(value: Value) -> Value {
    if value.is_object() {
        value
    } else {
        Value::Object(Default::default())
    }
}

fn has_hook(value: &Value) -> bool {
    let arr = match value
        .get("hooks")
        .and_then(|v| v.get(EVENT))
        .and_then(|v| v.as_array())
    {
        Some(a) => a,
        None => return false,
    };
    for entry in arr {
        let matcher = entry.get("matcher").and_then(|v| v.as_str()).unwrap_or("");
        if matcher != MATCHER {
            continue;
        }
        let inner = match entry.get("hooks").and_then(|v| v.as_array()) {
            Some(a) => a,
            None => continue,
        };
        for h in inner {
            if h.get("command").and_then(|v| v.as_str()) == Some(HOOK_COMMAND) {
                return true;
            }
        }
    }
    false
}

fn insert_hook(value: &mut Value) {
    let root = value.as_object_mut().expect("ensured object");
    let hooks = root
        .entry("hooks".to_string())
        .or_insert_with(|| Value::Object(Default::default()));
    let hooks_obj = hooks.as_object_mut().expect("hooks is object");
    let event = hooks_obj
        .entry(EVENT.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    let event_arr = event.as_array_mut().expect("event is array");
    event_arr.push(json!({
        "matcher": MATCHER,
        "hooks": [ { "type": "command", "command": HOOK_COMMAND } ]
    }));
}

fn remove_hook(value: &mut Value) -> bool {
    let mut removed = false;
    let Some(root) = value.as_object_mut() else {
        return false;
    };
    let Some(hooks_v) = root.get_mut("hooks") else {
        return false;
    };
    let Some(hooks_obj) = hooks_v.as_object_mut() else {
        return false;
    };
    let Some(event_v) = hooks_obj.get_mut(EVENT) else {
        return false;
    };
    let Some(arr) = event_v.as_array_mut() else {
        return false;
    };
    arr.retain_mut(|entry| {
        let matcher = entry.get("matcher").and_then(|v| v.as_str()).unwrap_or("");
        if matcher != MATCHER {
            return true;
        }
        let Some(inner_arr) = entry.get_mut("hooks").and_then(|v| v.as_array_mut()) else {
            return true;
        };
        let before = inner_arr.len();
        inner_arr.retain(|h| h.get("command").and_then(|v| v.as_str()) != Some(HOOK_COMMAND));
        if inner_arr.len() < before {
            removed = true;
        }
        !inner_arr.is_empty()
    });
    if arr.is_empty() {
        hooks_obj.remove(EVENT);
    }
    if hooks_obj.is_empty() {
        root.remove("hooks");
    }
    removed
}

fn write_with_backup(path: &Path, value: &Value) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;

    // Back up the existing file if present.  On Unix we open with O_NOFOLLOW to
    // refuse to follow a symlink — this prevents a symlink-swap attack where an
    // attacker replaces the settings file with a symlink to another file before
    // we copy it, causing us to overwrite an arbitrary user-owned file.
    if path.exists() {
        let bak = find_free_bak_path(path);
        backup_no_follow(path, &bak)
            .with_context(|| format!("backup {} -> {}", path.display(), bak.display()))?;
    }

    // Write new content via a temporary file in the same directory, then
    // atomically rename over the destination.  Using NamedTempFile::new_in
    // keeps the temp file on the same filesystem as the target, making the
    // rename a single kernel call (no cross-device copy).
    let pretty = serde_json::to_string_pretty(value)? + "\n";
    let mut tmp = NamedTempFile::new_in(parent)
        .with_context(|| format!("create tmp in {}", parent.display()))?;
    use std::io::Write as _;
    tmp.write_all(pretty.as_bytes())
        .with_context(|| "write tmp content")?;
    tmp.persist(path)
        .with_context(|| format!("rename tmp to {}", path.display()))?;
    Ok(())
}

/// Find the first free backup path: `.bak`, `.bak.1`, `.bak.2`, …
fn find_free_bak_path(path: &Path) -> PathBuf {
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("json");
    let base = path.with_extension(format!("{ext}.bak"));
    if !base.exists() {
        return base;
    }
    for n in 1u32.. {
        let candidate = path.with_extension(format!("{ext}.bak.{n}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    // Unreachable in practice — the loop is infinite.
    base
}

/// Copy `src` to `dst` while refusing to follow symlinks for `src` on Unix.
fn backup_no_follow(src: &Path, dst: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::io::Read as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        let mut f = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(src)
            .with_context(|| format!("open source {} (O_NOFOLLOW)", src.display()))?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)
            .with_context(|| format!("read source {}", src.display()))?;
        fs::write(dst, &buf).with_context(|| format!("write backup {}", dst.display()))?;
    }
    #[cfg(not(unix))]
    {
        // On Windows symlink attacks are less of a concern and O_NOFOLLOW is
        // unavailable; fall back to a plain copy.
        fs::copy(src, dst)
            .with_context(|| format!("copy {} -> {}", src.display(), dst.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn install_creates_file_with_hook() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.json");
        match install(&path).unwrap() {
            InstallOutcome::Installed => {}
            _ => panic!("expected Installed"),
        }
        let content = fs::read_to_string(&path).unwrap();
        let v: Value = serde_json::from_str(&content).unwrap();
        let arr = v["hooks"][EVENT].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["matcher"], MATCHER);
        assert_eq!(arr[0]["hooks"][0]["command"], HOOK_COMMAND);
    }

    #[test]
    fn install_is_idempotent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.json");
        install(&path).unwrap();
        match install(&path).unwrap() {
            InstallOutcome::AlreadyInstalled => {}
            _ => panic!("expected AlreadyInstalled"),
        }
    }

    #[test]
    fn install_preserves_existing_keys() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(
            &path,
            r#"{"theme":"dark","hooks":{"PreToolUse":[{"matcher":"Other","hooks":[{"type":"command","command":"foo"}]}]}}"#,
        )
        .unwrap();
        install(&path).unwrap();
        let v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["theme"], "dark");
        let arr = v["hooks"][EVENT].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        let backup = path.with_extension("json.bak");
        assert!(backup.exists());
    }

    #[test]
    fn uninstall_removes_only_our_hook() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(
            &path,
            r#"{"theme":"dark","hooks":{"PreToolUse":[{"matcher":"Other","hooks":[{"type":"command","command":"foo"}]}]}}"#,
        )
        .unwrap();
        install(&path).unwrap();
        match uninstall(&path).unwrap() {
            UninstallOutcome::Removed => {}
            _ => panic!("expected Removed"),
        }
        let v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["theme"], "dark");
        let arr = v["hooks"][EVENT].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["matcher"], "Other");
    }

    #[test]
    fn uninstall_nothing_to_do_returns_not_present() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, "{}").unwrap();
        match uninstall(&path).unwrap() {
            UninstallOutcome::NotPresent => {}
            _ => panic!("expected NotPresent"),
        }
    }

    /// H1: each install creates a new numbered backup instead of silently
    /// overwriting the previous one.
    #[test]
    fn backup_does_not_overwrite_previous() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.json");

        // First install — no backup yet.
        fs::write(&path, r#"{"v":1}"#).unwrap();
        install(&path).unwrap();
        let bak0 = path.with_extension("json.bak");
        assert!(bak0.exists(), ".bak should exist after first install");

        // Second install — settings file already updated; pre-place a fresh
        // original so install has something to modify.
        fs::write(&path, r#"{"v":2}"#).unwrap();
        install(&path).unwrap();
        let bak1 = path.with_extension("json.bak.1");
        assert!(bak1.exists(), ".bak.1 should exist after second install");

        // Content of the two backups must differ.
        let c0 = fs::read_to_string(&bak0).unwrap();
        let c1 = fs::read_to_string(&bak1).unwrap();
        assert_ne!(c0, c1, "backup files should have different content");
    }

    /// M1 (Unix only): `install` must refuse to follow a symlink at the
    /// settings path.  The target of the symlink must NOT be modified.
    #[cfg(unix)]
    #[test]
    fn install_does_not_follow_symlink() {
        use std::os::unix::fs::symlink;
        let dir = tempdir().unwrap();

        // Create the real target file (the "victim").
        let victim = dir.path().join("victim.json");
        fs::write(&victim, r#"{"untouched":true}"#).unwrap();

        // Place a symlink at the settings path pointing to the victim.
        let settings = dir.path().join("settings.json");
        symlink(&victim, &settings).unwrap();

        // install() should fail or handle the symlink gracefully — the
        // important invariant is that the victim is not modified.
        let _ = install(&settings);

        let victim_content = fs::read_to_string(&victim).unwrap();
        assert!(
            victim_content.contains("untouched"),
            "victim file must not be modified through symlink, got: {victim_content}"
        );
    }
}
