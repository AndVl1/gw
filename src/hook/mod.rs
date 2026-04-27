pub mod claude;
pub mod detect;

pub use claude::run as run_claude_hook;
pub use detect::detect_rewrite;
