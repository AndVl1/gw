//! Generic helpers for editing structured config files (JSON, JSONC, markdown).
//!
//! All agent-specific install/uninstall logic is delegated to `json_hook.rs`,
//! `rules.rs`, and `opencode.rs`. This module only owns:
//! - reading text/JSON files into a serde Value (or empty)
//! - atomic writes via a tempfile + rename
//! - rotating .bak backups (`.bak`, `.bak.1`, `.bak.2`, ...)
//! - O_NOFOLLOW protection against symlink-swap attacks on Unix

use anyhow::{Context, Result};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

pub fn read_text_or_empty(path: &Path) -> Result<String> {
    if !path.exists() {
        return Ok(String::new());
    }
    fs::read_to_string(path).with_context(|| format!("read {}", path.display()))
}

pub fn read_json_or_empty(path: &Path) -> Result<Value> {
    let content = read_text_or_empty(path)?;
    if content.trim().is_empty() {
        return Ok(Value::Object(Default::default()));
    }
    let value: Value = serde_json::from_str(&content)
        .with_context(|| format!("parse {} as JSON", path.display()))?;
    Ok(value)
}

pub fn ensure_object(value: Value) -> Value {
    if value.is_object() {
        value
    } else {
        Value::Object(Default::default())
    }
}

pub fn write_json_with_backup(path: &Path, value: &Value) -> Result<()> {
    let pretty = serde_json::to_string_pretty(value)? + "\n";
    write_text_with_backup(path, &pretty)
}

pub fn write_text_with_backup(path: &Path, content: &str) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;

    if path.exists() {
        let bak = find_free_bak_path(path);
        backup_no_follow(path, &bak)
            .with_context(|| format!("backup {} -> {}", path.display(), bak.display()))?;
    }

    let mut tmp = NamedTempFile::new_in(parent)
        .with_context(|| format!("create tmp in {}", parent.display()))?;
    use std::io::Write as _;
    tmp.write_all(content.as_bytes())
        .with_context(|| "write tmp content")?;
    tmp.persist(path)
        .with_context(|| format!("rename tmp to {}", path.display()))?;
    Ok(())
}

pub fn find_free_bak_path(path: &Path) -> PathBuf {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_default();
    let bak_ext = if ext.is_empty() {
        "bak".to_string()
    } else {
        format!("{ext}.bak")
    };
    let base = path.with_extension(&bak_ext);
    if !base.exists() {
        return base;
    }
    for n in 1u32.. {
        let candidate = path.with_extension(format!("{bak_ext}.{n}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    base
}

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
    fn write_creates_file_and_backs_up_existing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.json");
        write_text_with_backup(&path, "v1").unwrap();
        write_text_with_backup(&path, "v2").unwrap();
        let bak = path.with_extension("json.bak");
        assert!(bak.exists());
        assert_eq!(fs::read_to_string(&bak).unwrap(), "v1");
        assert_eq!(fs::read_to_string(&path).unwrap(), "v2");
    }

    #[test]
    fn rotates_bak_numbers() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("a.json");
        write_text_with_backup(&path, "1").unwrap();
        write_text_with_backup(&path, "2").unwrap();
        write_text_with_backup(&path, "3").unwrap();
        assert!(path.with_extension("json.bak").exists());
        assert!(path.with_extension("json.bak.1").exists());
    }

    #[cfg(unix)]
    #[test]
    fn no_follow_prevents_symlink_swap() {
        use std::os::unix::fs::symlink;
        let dir = tempdir().unwrap();
        let victim = dir.path().join("victim.json");
        fs::write(&victim, "untouched").unwrap();
        let settings = dir.path().join("settings.json");
        symlink(&victim, &settings).unwrap();
        let _ = write_text_with_backup(&settings, "evil");
        // Either the write fails or the file is replaced atomically without
        // touching the victim through the symlink. The invariant is: victim
        // contents must remain "untouched".
        let v = fs::read_to_string(&victim).unwrap();
        assert_eq!(v, "untouched");
    }
}
