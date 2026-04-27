use anyhow::Result;
use serde_json::{json, Value};
use std::io::Read;

use super::detect::detect_rewrite;

pub fn run() -> Result<i32> {
    // Read raw bytes first; lossy-convert so non-UTF8 input does not cause a
    // non-zero exit.  Claude Code interprets a non-zero hook exit as "block the
    // tool", which would be wrong for a simple encoding edge case.
    let mut raw = Vec::new();
    std::io::stdin().read_to_end(&mut raw)?;
    let input = String::from_utf8_lossy(&raw);

    let parsed: Value = match serde_json::from_str(input.as_ref()) {
        Ok(v) => v,
        // Malformed JSON — exit 0 silently so we don't block any tool.
        Err(_) => return Ok(0),
    };
    let cmd = parsed
        .get("tool_input")
        .and_then(|v| v.get("command"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let Some(rewritten) = detect_rewrite(cmd) else {
        return Ok(0);
    };
    let out = json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "permissionDecisionReason": "gw filter (auto-wrap gradlew)",
            "updatedInput": { "command": rewritten }
        }
    });
    println!("{}", out);
    Ok(0)
}
