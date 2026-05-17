// Legacy hook constants used by Claude Code integration.
// Other agents have their own constants (see json_hook.rs).
pub const CLAUDE_HOOK_COMMAND: &str = "gw hook claude-code";
pub const CLAUDE_MATCHER: &str = "Bash";
pub const CLAUDE_EVENT: &str = "PreToolUse";

// Backward-compat hook command — early gw versions registered "gw hook claude".
// Detect both during idempotency / uninstall checks so older installs are
// recognized and cleaned up properly.
pub const CLAUDE_HOOK_COMMAND_LEGACY: &str = "gw hook claude";

// Markers used to delimit the gw-managed block inside markdown rule files.
pub const MARKER_BEGIN: &str = "<!-- gw:begin -->";
pub const MARKER_END: &str = "<!-- gw:end -->";

/// Body for rules-based agents (no hooks). The agent reads this as instruction.
pub const RULE_BODY: &str = r#"## Build Commands (managed by gw — do not edit between markers)

When running Gradle, always wrap with `gw`:

- `gw ./gradlew assembleDebug` (correct)
- `./gradlew assembleDebug` (avoid — full noisy log floods the context)

`gw` filters Gradle output (~70% token reduction). Same exit codes. Errors, warnings, and BUILD SUCCESSFUL/FAILED are preserved verbatim. A heartbeat line prints every 30 s during silent stretches so you know the build is alive.

**Do not pipe `gw` through `tail`, `head`, or similar truncators.** The output is already filtered: a reflexive `| tail -n 80` will drop the leading error/stacktrace and leave you with only the trailing summary. Read the full `gw` output directly.

**When delegating a Gradle build to a subagent, restate this rule in the delegation prompt.** Subagents do not inherit this file's context automatically; without an explicit instruction they will reflexively append `| tail`/`| head` and the truncated output will hide the actual failure.
"#;

/// Body for hook-based agents (Claude/Gemini/Cursor) — short note that
/// interception is automatic. Saved to companion docs file (CLAUDE.md / GEMINI.md / AGENTS.md).
pub const RULE_BODY_HOOK_NOTE: &str = r#"## Build Commands (managed by gw — auto-intercepted)

Gradle commands are automatically wrapped with `gw` via the configured PreToolUse hook — no manual action needed.

`gw` filters Gradle output (~70% token reduction): errors, warnings, status lines preserved verbatim; daemon noise, downloads, configure phase, and per-task lines are dropped. Heartbeat fires every 30 s during silent stretches.

**Do not pipe `gw` through `tail`, `head`, or similar truncators.** The output is already filtered: a reflexive `| tail -n 80` will drop the leading error/stacktrace and leave you with only the trailing summary. Read the full `gw` output directly.

**When delegating a Gradle build to a subagent, restate this rule in the delegation prompt.** Subagents do not inherit this file's context automatically; without an explicit instruction they will reflexively append `| tail`/`| head` and the truncated output will hide the actual failure.
"#;
