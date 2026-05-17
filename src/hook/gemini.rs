//! Gemini CLI BeforeTool hook entry.
//!
//! Stdin schema (Gemini CLI):
//! ```json
//! { "tool_name": "bash", "tool_input": { "command": "..." }, ... }
//! ```
//! Stdout: similar JSON shape with `decision: "allow"`, optional rewrite via
//! `tool_input` mutation. Spec across versions is unstable — we mirror the
//! Claude-style envelope which Gemini CLI's recent docs describe as compatible.

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
    let mut msg = String::from("gw filter (auto-wrap gradlew)");
    if let Some(warn) = detect_truncation(cmd) {
        msg.push_str(" — warning: ");
        msg.push_str(warn);
    }
    let out = json!({
        "decision": "allow",
        "continue": true,
        "tool_input": { "command": rewritten },
        "systemMessage": msg
    });
    println!("{}", out);
    Ok(0)
}
