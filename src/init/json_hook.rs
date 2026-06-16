//! JSON-settings hook installation for hook-capable agents.
//!
//! Three different schemas live here, deliberately kept separate:
//! - Claude Code: `hooks.PreToolUse[].matcher == "Bash"` with inner `hooks[].command`.
//! - Gemini CLI: `hooks.BeforeTool[]` with `matcher: { tool: "bash" }` and top-level `command`.
//! - Cursor: `hooks.beforeShellExecution[]` with top-level `command` (no matcher).
//!
//! All three use the shared `settings` helpers for atomic writes and `.bak` rotation.

use anyhow::Result;
use serde_json::{json, Value};
use std::path::Path;

use super::agent::{AgentStatus, InstallOutcome, UninstallOutcome};
use super::consts::{
    CLAUDE_EVENT, CLAUDE_HOOK_COMMAND, CLAUDE_HOOK_COMMAND_LEGACY, CLAUDE_MATCHER,
};
use super::settings::{ensure_object, read_json_or_empty, write_json_with_backup};

// ============================================================================
// Claude Code
// ============================================================================

const CLAUDE_HOOK_COMMANDS: &[&str] = &[CLAUDE_HOOK_COMMAND, CLAUDE_HOOK_COMMAND_LEGACY];

pub fn status_claude(path: &Path) -> Result<AgentStatus> {
    if !path.exists() {
        return Ok(AgentStatus::NoFile);
    }
    let value = ensure_object(read_json_or_empty(path)?);
    Ok(claude_status(&value))
}

fn claude_status(value: &Value) -> AgentStatus {
    let arr = match value
        .get("hooks")
        .and_then(|v| v.get(CLAUDE_EVENT))
        .and_then(|v| v.as_array())
    {
        Some(a) => a,
        None => return AgentStatus::NotInstalled,
    };
    let mut found_legacy = false;
    for entry in arr {
        if entry.get("matcher").and_then(|v| v.as_str()) != Some(CLAUDE_MATCHER) {
            continue;
        }
        let inner = match entry.get("hooks").and_then(|v| v.as_array()) {
            Some(a) => a,
            None => continue,
        };
        for h in inner {
            let cmd = h.get("command").and_then(|v| v.as_str()).unwrap_or("");
            if cmd == CLAUDE_HOOK_COMMAND {
                return AgentStatus::Installed;
            }
            if cmd == CLAUDE_HOOK_COMMAND_LEGACY {
                found_legacy = true;
            }
        }
    }
    if found_legacy {
        AgentStatus::InstalledLegacy
    } else {
        AgentStatus::NotInstalled
    }
}

pub fn install_claude(path: &Path) -> Result<InstallOutcome> {
    let mut value = ensure_object(read_json_or_empty(path)?);
    match claude_status(&value) {
        AgentStatus::Installed => {
            if claude_current_hook_at_front(&value) {
                return Ok(InstallOutcome::AlreadyInstalled);
            }
            // Current hook exists but is not at index 0 — move it to the front so
            // it wins over any other Bash PreToolUse hook (e.g. rtk).
            claude_remove_hook(&mut value);
            claude_insert_hook(&mut value);
            write_json_with_backup(path, &value)?;
            Ok(InstallOutcome::Installed)
        }
        AgentStatus::InstalledLegacy => {
            // Legacy "gw hook claude" found — replace it with the current command
            // at index 0 so the migration is transparent to the user.
            claude_remove_hook(&mut value);
            claude_insert_hook(&mut value);
            write_json_with_backup(path, &value)?;
            Ok(InstallOutcome::Installed)
        }
        AgentStatus::NotInstalled | AgentStatus::NoFile => {
            claude_insert_hook(&mut value);
            write_json_with_backup(path, &value)?;
            Ok(InstallOutcome::Installed)
        }
    }
}

pub fn uninstall_claude(path: &Path) -> Result<UninstallOutcome> {
    if !path.exists() {
        return Ok(UninstallOutcome::NoFile);
    }
    let mut value = ensure_object(read_json_or_empty(path)?);
    if !claude_remove_hook(&mut value) {
        return Ok(UninstallOutcome::NotPresent);
    }
    write_json_with_backup(path, &value)?;
    Ok(UninstallOutcome::Removed)
}

/// Returns `true` when the first entry in the PreToolUse array is gw's
/// current-command Bash hook. Used by `install_claude` to decide whether an
/// already-installed hook still needs to be moved to the front of the array.
fn claude_current_hook_at_front(value: &Value) -> bool {
    let Some(arr) = value
        .get("hooks")
        .and_then(|v| v.get(CLAUDE_EVENT))
        .and_then(|v| v.as_array())
    else {
        return false;
    };
    let Some(first) = arr.first() else {
        return false;
    };
    if first.get("matcher").and_then(|v| v.as_str()) != Some(CLAUDE_MATCHER) {
        return false;
    }
    let Some(inner) = first.get("hooks").and_then(|v| v.as_array()) else {
        return false;
    };
    inner
        .iter()
        .any(|h| h.get("command").and_then(|v| v.as_str()) == Some(CLAUDE_HOOK_COMMAND))
}

fn claude_insert_hook(value: &mut Value) {
    let root = value.as_object_mut().expect("ensured object");
    let hooks = root
        .entry("hooks".to_string())
        .or_insert_with(|| Value::Object(Default::default()));
    let hooks_obj = hooks.as_object_mut().expect("hooks is object");
    let event = hooks_obj
        .entry(CLAUDE_EVENT.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    let event_arr = event.as_array_mut().expect("event is array");
    // Insert at index 0 so gw's hook runs before any other Bash PreToolUse
    // hook (e.g. rtk). For non-gradle commands gw exits 0 with no output, so
    // other hooks are unaffected.
    event_arr.insert(
        0,
        json!({
            "matcher": CLAUDE_MATCHER,
            "hooks": [ { "type": "command", "command": CLAUDE_HOOK_COMMAND } ]
        }),
    );
}

fn claude_remove_hook(value: &mut Value) -> bool {
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
    let Some(event_v) = hooks_obj.get_mut(CLAUDE_EVENT) else {
        return false;
    };
    let Some(arr) = event_v.as_array_mut() else {
        return false;
    };
    arr.retain_mut(|entry| {
        if entry.get("matcher").and_then(|v| v.as_str()) != Some(CLAUDE_MATCHER) {
            return true;
        }
        let Some(inner_arr) = entry.get_mut("hooks").and_then(|v| v.as_array_mut()) else {
            return true;
        };
        let before = inner_arr.len();
        inner_arr.retain(|h| {
            let cmd = h.get("command").and_then(|v| v.as_str()).unwrap_or("");
            !CLAUDE_HOOK_COMMANDS.contains(&cmd)
        });
        if inner_arr.len() < before {
            removed = true;
        }
        !inner_arr.is_empty()
    });
    if arr.is_empty() {
        hooks_obj.remove(CLAUDE_EVENT);
    }
    if hooks_obj.is_empty() {
        root.remove("hooks");
    }
    removed
}

/// Returns the commands of non-gw PreToolUse hooks that use the Bash matcher.
///
/// These hooks compete for the same tool events as gw and may intercept gradle
/// commands first if they appear earlier in the array. The result is used by
/// `gw doctor` to warn the user about potential conflicts; no auto-fix is
/// performed here.
pub fn conflicts_claude(path: &Path) -> Result<Vec<String>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let value = ensure_object(read_json_or_empty(path)?);
    let Some(arr) = value
        .get("hooks")
        .and_then(|v| v.get(CLAUDE_EVENT))
        .and_then(|v| v.as_array())
    else {
        return Ok(Vec::new());
    };
    let mut conflicts = Vec::new();
    for entry in arr {
        if entry.get("matcher").and_then(|v| v.as_str()) != Some(CLAUDE_MATCHER) {
            continue;
        }
        let Some(inner) = entry.get("hooks").and_then(|v| v.as_array()) else {
            continue;
        };
        for h in inner {
            let cmd = h.get("command").and_then(|v| v.as_str()).unwrap_or("");
            if !CLAUDE_HOOK_COMMANDS.contains(&cmd) {
                conflicts.push(cmd.to_string());
            }
        }
    }
    Ok(conflicts)
}

// ============================================================================
// Gemini CLI
// ============================================================================

const GEMINI_EVENT: &str = "BeforeTool";
const GEMINI_HOOK_COMMAND: &str = "gw hook gemini-cli";

pub fn status_gemini(path: &Path) -> Result<AgentStatus> {
    if !path.exists() {
        return Ok(AgentStatus::NoFile);
    }
    let value = ensure_object(read_json_or_empty(path)?);
    if gemini_has_hook(&value) {
        Ok(AgentStatus::Installed)
    } else {
        Ok(AgentStatus::NotInstalled)
    }
}

pub fn install_gemini(path: &Path) -> Result<InstallOutcome> {
    let mut value = ensure_object(read_json_or_empty(path)?);
    if gemini_has_hook(&value) {
        return Ok(InstallOutcome::AlreadyInstalled);
    }
    gemini_insert_hook(&mut value);
    write_json_with_backup(path, &value)?;
    Ok(InstallOutcome::Installed)
}

pub fn uninstall_gemini(path: &Path) -> Result<UninstallOutcome> {
    if !path.exists() {
        return Ok(UninstallOutcome::NoFile);
    }
    let mut value = ensure_object(read_json_or_empty(path)?);
    if !gemini_remove_hook(&mut value) {
        return Ok(UninstallOutcome::NotPresent);
    }
    write_json_with_backup(path, &value)?;
    Ok(UninstallOutcome::Removed)
}

fn gemini_has_hook(value: &Value) -> bool {
    let Some(arr) = value
        .get("hooks")
        .and_then(|v| v.get(GEMINI_EVENT))
        .and_then(|v| v.as_array())
    else {
        return false;
    };
    arr.iter()
        .any(|h| h.get("command").and_then(|v| v.as_str()) == Some(GEMINI_HOOK_COMMAND))
}

fn gemini_insert_hook(value: &mut Value) {
    let root = value.as_object_mut().expect("ensured object");
    let hooks = root
        .entry("hooks".to_string())
        .or_insert_with(|| Value::Object(Default::default()));
    let hooks_obj = hooks.as_object_mut().expect("hooks is object");
    let event = hooks_obj
        .entry(GEMINI_EVENT.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    let event_arr = event.as_array_mut().expect("event is array");
    event_arr.push(json!({
        "matcher": { "tool": "bash" },
        "type": "command",
        "command": GEMINI_HOOK_COMMAND
    }));
}

fn gemini_remove_hook(value: &mut Value) -> bool {
    let Some(root) = value.as_object_mut() else {
        return false;
    };
    let Some(hooks_v) = root.get_mut("hooks") else {
        return false;
    };
    let Some(hooks_obj) = hooks_v.as_object_mut() else {
        return false;
    };
    let Some(event_v) = hooks_obj.get_mut(GEMINI_EVENT) else {
        return false;
    };
    let Some(arr) = event_v.as_array_mut() else {
        return false;
    };
    let before = arr.len();
    arr.retain(|h| h.get("command").and_then(|v| v.as_str()) != Some(GEMINI_HOOK_COMMAND));
    let removed = arr.len() < before;
    if arr.is_empty() {
        hooks_obj.remove(GEMINI_EVENT);
    }
    if hooks_obj.is_empty() {
        root.remove("hooks");
    }
    removed
}

// ============================================================================
// Cursor
// ============================================================================

const CURSOR_EVENT: &str = "beforeShellExecution";
const CURSOR_HOOK_COMMAND: &str = "gw hook cursor";
const CURSOR_SCHEMA: &str = "https://unpkg.com/cursor-hooks@latest/schema/hooks.schema.json";

pub fn status_cursor(path: &Path) -> Result<AgentStatus> {
    if !path.exists() {
        return Ok(AgentStatus::NoFile);
    }
    let value = ensure_object(read_json_or_empty(path)?);
    if cursor_has_hook(&value) {
        Ok(AgentStatus::Installed)
    } else {
        Ok(AgentStatus::NotInstalled)
    }
}

pub fn install_cursor(path: &Path) -> Result<InstallOutcome> {
    let mut value = ensure_object(read_json_or_empty(path)?);
    if cursor_has_hook(&value) {
        return Ok(InstallOutcome::AlreadyInstalled);
    }
    cursor_insert_hook(&mut value);
    write_json_with_backup(path, &value)?;
    Ok(InstallOutcome::Installed)
}

pub fn uninstall_cursor(path: &Path) -> Result<UninstallOutcome> {
    if !path.exists() {
        return Ok(UninstallOutcome::NoFile);
    }
    let mut value = ensure_object(read_json_or_empty(path)?);
    if !cursor_remove_hook(&mut value) {
        return Ok(UninstallOutcome::NotPresent);
    }
    write_json_with_backup(path, &value)?;
    Ok(UninstallOutcome::Removed)
}

fn cursor_has_hook(value: &Value) -> bool {
    let Some(arr) = value
        .get("hooks")
        .and_then(|v| v.get(CURSOR_EVENT))
        .and_then(|v| v.as_array())
    else {
        return false;
    };
    arr.iter()
        .any(|h| h.get("command").and_then(|v| v.as_str()) == Some(CURSOR_HOOK_COMMAND))
}

fn cursor_insert_hook(value: &mut Value) {
    let root = value.as_object_mut().expect("ensured object");
    root.entry("$schema".to_string())
        .or_insert_with(|| Value::String(CURSOR_SCHEMA.to_string()));
    root.entry("version".to_string())
        .or_insert_with(|| Value::Number(1.into()));
    let hooks = root
        .entry("hooks".to_string())
        .or_insert_with(|| Value::Object(Default::default()));
    let hooks_obj = hooks.as_object_mut().expect("hooks is object");
    let event = hooks_obj
        .entry(CURSOR_EVENT.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    let event_arr = event.as_array_mut().expect("event is array");
    event_arr.push(json!({
        "type": "command",
        "command": CURSOR_HOOK_COMMAND
    }));
}

fn cursor_remove_hook(value: &mut Value) -> bool {
    let Some(root) = value.as_object_mut() else {
        return false;
    };
    let Some(hooks_v) = root.get_mut("hooks") else {
        return false;
    };
    let Some(hooks_obj) = hooks_v.as_object_mut() else {
        return false;
    };
    let Some(event_v) = hooks_obj.get_mut(CURSOR_EVENT) else {
        return false;
    };
    let Some(arr) = event_v.as_array_mut() else {
        return false;
    };
    let before = arr.len();
    arr.retain(|h| h.get("command").and_then(|v| v.as_str()) != Some(CURSOR_HOOK_COMMAND));
    let removed = arr.len() < before;
    if arr.is_empty() {
        hooks_obj.remove(CURSOR_EVENT);
    }
    if hooks_obj.is_empty() {
        root.remove("hooks");
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn claude_install_creates_hook_and_is_idempotent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.json");
        assert!(matches!(
            install_claude(&path).unwrap(),
            InstallOutcome::Installed
        ));
        assert!(matches!(
            install_claude(&path).unwrap(),
            InstallOutcome::AlreadyInstalled
        ));
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let arr = v["hooks"][CLAUDE_EVENT].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["matcher"], CLAUDE_MATCHER);
        assert_eq!(arr[0]["hooks"][0]["command"], CLAUDE_HOOK_COMMAND);
    }

    #[test]
    fn claude_migrates_legacy_command_on_install() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"gw hook claude"}]}]}}"#,
        )
        .unwrap();
        // Legacy entry must be replaced with the current command, not
        // silently treated as already-installed.
        assert!(matches!(
            install_claude(&path).unwrap(),
            InstallOutcome::Installed
        ));
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let arr = v["hooks"][CLAUDE_EVENT].as_array().unwrap();
        // Legacy entry gone, current entry at index 0.
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["hooks"][0]["command"], CLAUDE_HOOK_COMMAND);
    }

    #[test]
    fn claude_uninstall_strips_only_our_hook() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"theme":"dark","hooks":{"PreToolUse":[{"matcher":"Other","hooks":[{"type":"command","command":"foo"}]}]}}"#,
        )
        .unwrap();
        install_claude(&path).unwrap();
        assert!(matches!(
            uninstall_claude(&path).unwrap(),
            UninstallOutcome::Removed
        ));
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["theme"], "dark");
        let arr = v["hooks"][CLAUDE_EVENT].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["matcher"], "Other");
    }

    #[test]
    fn gemini_install_creates_hook_and_is_idempotent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.json");
        install_gemini(&path).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let arr = v["hooks"][GEMINI_EVENT].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["matcher"]["tool"], "bash");
        assert_eq!(arr[0]["command"], GEMINI_HOOK_COMMAND);
        assert!(matches!(
            install_gemini(&path).unwrap(),
            InstallOutcome::AlreadyInstalled
        ));
    }

    #[test]
    fn gemini_uninstall_removes_block() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.json");
        install_gemini(&path).unwrap();
        assert!(matches!(
            uninstall_gemini(&path).unwrap(),
            UninstallOutcome::Removed
        ));
    }

    #[test]
    fn cursor_install_writes_schema_and_hook() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("hooks.json");
        install_cursor(&path).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["version"], 1);
        assert!(v["$schema"].as_str().unwrap().contains("cursor-hooks"));
        let arr = v["hooks"][CURSOR_EVENT].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["command"], CURSOR_HOOK_COMMAND);
    }

    #[test]
    fn cursor_install_is_idempotent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("hooks.json");
        install_cursor(&path).unwrap();
        assert!(matches!(
            install_cursor(&path).unwrap(),
            InstallOutcome::AlreadyInstalled
        ));
    }
}
