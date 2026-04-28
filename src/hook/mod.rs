pub mod claude;
pub mod cursor;
pub mod detect;
pub mod gemini;

pub use detect::detect_rewrite;

use anyhow::Result;

/// Dispatch `gw hook <name>` to the right per-agent entry.
/// Accepts canonical names (`claude-code`, `gemini-cli`, `cursor`) and the
/// legacy alias `claude` (early gw versions registered the hook as `gw hook claude`).
pub fn dispatch(name: &str) -> Result<i32> {
    match name {
        "claude-code" | "claude" => claude::run(),
        "gemini-cli" | "gemini" => gemini::run(),
        "cursor" => cursor::run(),
        other => {
            eprintln!("gw hook: unknown agent: {other}");
            Ok(2)
        }
    }
}
