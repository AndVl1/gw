use anyhow::Result;
use serde_json::{json, Value};
use std::io::Read;

use super::detect::{detect_rewrite, detect_truncation};

pub fn run() -> Result<i32> {
    let mut raw = Vec::new();
    std::io::stdin().read_to_end(&mut raw)?;
    let input = String::from_utf8_lossy(&raw);

    let parsed: Value = match serde_json::from_str(input.as_ref()) {
        Ok(v) => v,
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
    let mut reason = String::from("gw filter (auto-wrap gradlew)");
    if let Some(warn) = detect_truncation(cmd) {
        reason.push_str(" — warning: ");
        reason.push_str(warn);
    }
    let out = json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "permissionDecisionReason": reason,
            "updatedInput": { "command": rewritten }
        }
    });
    println!("{}", out);
    Ok(0)
}
