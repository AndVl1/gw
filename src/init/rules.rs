//! Generic marker-block install/uninstall for markdown rules files.
//!
//! Used by all rules-based agents (codex/cline/windsurf/kilocode/antigravity/copilot)
//! and as a companion docs note for hook-based agents.
//!
//! Block format:
//!
//! ```markdown
//! <!-- gw:begin -->
//! ## Build Commands ...
//! <!-- gw:end -->
//! ```
//!
//! Install: append marker block to file (creating file if absent). Idempotent —
//! detects existing block by both markers and returns AlreadyInstalled.
//! Uninstall: strips the block plus a single surrounding blank line, leaves
//! everything else untouched.

use anyhow::{anyhow, Result};
use std::path::Path;

use super::agent::{InstallOutcome, UninstallOutcome};
use super::consts::{MARKER_BEGIN, MARKER_END};
use super::settings::{read_text_or_empty, write_text_with_backup};

pub fn install(path: &Path, body: &str) -> Result<InstallOutcome> {
    let existing = read_text_or_empty(path)?;
    if find_block(&existing).is_some() {
        return Ok(InstallOutcome::AlreadyInstalled);
    }
    let block = render_block(body);
    let new_content = if existing.is_empty() {
        format!("{block}\n")
    } else if existing.ends_with("\n\n") {
        format!("{existing}{block}\n")
    } else if existing.ends_with('\n') {
        format!("{existing}\n{block}\n")
    } else {
        format!("{existing}\n\n{block}\n")
    };
    write_text_with_backup(path, &new_content)?;
    Ok(InstallOutcome::Installed)
}

pub fn uninstall(path: &Path) -> Result<UninstallOutcome> {
    if !path.exists() {
        return Ok(UninstallOutcome::NoFile);
    }
    let existing = read_text_or_empty(path)?;
    let Some((start, end)) = find_block(&existing) else {
        return Ok(UninstallOutcome::NotPresent);
    };

    // Trim a single leading blank-line separator if present, so we don't leave
    // an awkward gap when the block sat between two paragraphs.
    let trim_start = match existing[..start].rfind('\n') {
        Some(prev_nl) if existing[..prev_nl].trim_end_matches('\n').len() != prev_nl => {
            // Two consecutive newlines before block — drop one.
            existing[..start]
                .rfind("\n\n")
                .map(|p| p + 1)
                .unwrap_or(start)
        }
        _ => start,
    };

    let mut next = String::with_capacity(existing.len());
    next.push_str(&existing[..trim_start]);
    let after = &existing[end..];
    next.push_str(after.strip_prefix('\n').unwrap_or(after));

    if next.trim().is_empty() {
        // File would be empty/whitespace only — remove it entirely so we leave
        // no trace.  This matches user expectation that uninstall is a clean undo.
        std::fs::remove_file(path).map_err(|e| anyhow!("remove {} failed: {e}", path.display()))?;
    } else {
        write_text_with_backup(path, &next)?;
    }
    Ok(UninstallOutcome::Removed)
}

fn render_block(body: &str) -> String {
    format!("{MARKER_BEGIN}\n{}{MARKER_END}", ensure_trailing_nl(body))
}

fn ensure_trailing_nl(s: &str) -> String {
    if s.ends_with('\n') {
        s.to_string()
    } else {
        format!("{s}\n")
    }
}

/// Find the byte range of an existing block (start..end), inclusive of markers.
fn find_block(content: &str) -> Option<(usize, usize)> {
    let begin = content.find(MARKER_BEGIN)?;
    let after_begin = begin + MARKER_BEGIN.len();
    let rel_end = content[after_begin..].find(MARKER_END)?;
    Some((begin, after_begin + rel_end + MARKER_END.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    const BODY: &str = "## test\n\ncontent line\n";

    #[test]
    fn install_creates_file_when_absent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("AGENTS.md");
        assert!(matches!(
            install(&path, BODY).unwrap(),
            InstallOutcome::Installed
        ));
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains(MARKER_BEGIN));
        assert!(content.contains(MARKER_END));
        assert!(content.contains("content line"));
    }

    #[test]
    fn install_appends_block_to_existing_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("AGENTS.md");
        std::fs::write(&path, "# Project rules\n\nUse strict types.\n").unwrap();
        install(&path, BODY).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.starts_with("# Project rules"));
        assert!(content.contains(MARKER_BEGIN));
        let bak = path.with_extension("md.bak");
        assert!(bak.exists());
    }

    #[test]
    fn install_is_idempotent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("CLAUDE.md");
        install(&path, BODY).unwrap();
        assert!(matches!(
            install(&path, BODY).unwrap(),
            InstallOutcome::AlreadyInstalled
        ));
    }

    #[test]
    fn uninstall_strips_only_our_block() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("AGENTS.md");
        std::fs::write(&path, "# Project rules\n\nUse strict types.\n").unwrap();
        install(&path, BODY).unwrap();
        assert!(matches!(
            uninstall(&path).unwrap(),
            UninstallOutcome::Removed
        ));
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("Project rules"));
        assert!(!content.contains(MARKER_BEGIN));
        assert!(!content.contains("content line"));
    }

    #[test]
    fn uninstall_removes_file_if_only_block_remained() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("AGENTS.md");
        install(&path, BODY).unwrap();
        uninstall(&path).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn uninstall_no_file_returns_no_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("missing.md");
        assert!(matches!(
            uninstall(&path).unwrap(),
            UninstallOutcome::NoFile
        ));
    }

    #[test]
    fn uninstall_no_block_returns_not_present() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("AGENTS.md");
        std::fs::write(&path, "# Just rules\n").unwrap();
        assert!(matches!(
            uninstall(&path).unwrap(),
            UninstallOutcome::NotPresent
        ));
    }
}
