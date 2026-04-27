//! Cursor beforeShellExecution hook entry.
//!
//! Stdin schema (per cursor.com/docs/hooks):
//! ```json
//! { "command": "...", "cwd": "...", "hook_event_name": "beforeShellExecution", ... }
//! ```
//! Stdout: `{ "continue": bool, "permission": "allow"|"deny"|"ask",
//!            "userMessage"?, "agentMessage"? }`
//!
//! Cursor has no in-band rewrite mechanism — the hook can only allow/deny.
//! For Gradle commands we therefore deny with a `userMessage` instructing the
//! agent to retry with `gw` prefixed, mirroring rtk's approach.

use anyhow::Result;
use serde_json::{json, Value};
use std::io::Read;

use super::detect::detect_rewrite;

pub fn run() -> Result<i32> {
    let mut raw = Vec::new();
    std::io::stdin().read_to_end(&mut raw)?;
    let input = String::from_utf8_lossy(&raw);

    let parsed: Value = match serde_json::from_str(input.as_ref()) {
        Ok(v) => v,
        Err(_) => return Ok(0),
    };
    let cmd = parsed.get("command").and_then(|v| v.as_str()).unwrap_or("");
    let Some(rewritten) = detect_rewrite(cmd) else {
        // Allow — not our concern.
        return Ok(0);
    };

    let out = json!({
        "continue": false,
        "permission": "deny",
        "agentMessage": format!(
            "Wrap Gradle commands with gw to filter noisy output. Retry: {rewritten}"
        ),
        "userMessage": "gw filter requested rewrite"
    });
    println!("{}", out);
    Ok(0)
}
