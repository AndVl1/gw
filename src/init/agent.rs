use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Agent {
    ClaudeCode,
    GeminiCli,
    Cursor,
    Codex,
    Cline,
    Windsurf,
    Kilocode,
    Antigravity,
    Opencode,
    Copilot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Global,
    Local,
}

#[derive(Debug, Clone, Copy)]
pub enum IntegrationKind {
    ClaudeHook,
    GeminiHook,
    CursorHook,
    OpencodePlugin,
    RulesAppend,
}

impl Agent {
    pub const ALL: &'static [Agent] = &[
        Agent::ClaudeCode,
        Agent::GeminiCli,
        Agent::Cursor,
        Agent::Codex,
        Agent::Cline,
        Agent::Windsurf,
        Agent::Kilocode,
        Agent::Antigravity,
        Agent::Opencode,
        Agent::Copilot,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Agent::ClaudeCode => "claude-code",
            Agent::GeminiCli => "gemini-cli",
            Agent::Cursor => "cursor",
            Agent::Codex => "codex",
            Agent::Cline => "cline",
            Agent::Windsurf => "windsurf",
            Agent::Kilocode => "kilocode",
            Agent::Antigravity => "antigravity",
            Agent::Opencode => "opencode",
            Agent::Copilot => "copilot",
        }
    }

    pub fn display(self) -> &'static str {
        match self {
            Agent::ClaudeCode => "Claude Code",
            Agent::GeminiCli => "Gemini CLI",
            Agent::Cursor => "Cursor",
            Agent::Codex => "Codex",
            Agent::Cline => "Cline / Roo Code",
            Agent::Windsurf => "Windsurf",
            Agent::Kilocode => "Kilo Code",
            Agent::Antigravity => "Google Antigravity",
            Agent::Opencode => "OpenCode",
            Agent::Copilot => "GitHub Copilot",
        }
    }

    pub fn kind(self) -> IntegrationKind {
        match self {
            Agent::ClaudeCode => IntegrationKind::ClaudeHook,
            Agent::GeminiCli => IntegrationKind::GeminiHook,
            Agent::Cursor => IntegrationKind::CursorHook,
            Agent::Opencode => IntegrationKind::OpencodePlugin,
            Agent::Codex
            | Agent::Cline
            | Agent::Windsurf
            | Agent::Kilocode
            | Agent::Antigravity
            | Agent::Copilot => IntegrationKind::RulesAppend,
        }
    }

    pub fn from_key(s: &str) -> Option<Agent> {
        Agent::ALL.iter().copied().find(|a| a.key() == s)
    }

    /// Primary integration file path for the chosen scope.
    pub fn path(self, scope: Scope) -> Option<PathBuf> {
        match (self, scope) {
            (Agent::ClaudeCode, Scope::Global) => home(".claude/settings.json"),
            (Agent::ClaudeCode, Scope::Local) => Some(PathBuf::from(".claude/settings.local.json")),

            (Agent::GeminiCli, Scope::Global) => home(".gemini/settings.json"),
            (Agent::GeminiCli, Scope::Local) => Some(PathBuf::from(".gemini/settings.json")),

            (Agent::Cursor, Scope::Global) => home(".cursor/hooks.json"),
            (Agent::Cursor, Scope::Local) => Some(PathBuf::from(".cursor/hooks.json")),

            (Agent::Codex, Scope::Global) => home(".codex/AGENTS.md"),
            (Agent::Codex, Scope::Local) => Some(PathBuf::from("AGENTS.md")),

            (Agent::Cline, Scope::Local) => Some(PathBuf::from(".clinerules")),
            (Agent::Cline, Scope::Global) => None,

            (Agent::Windsurf, Scope::Local) => Some(PathBuf::from(".windsurfrules")),
            (Agent::Windsurf, Scope::Global) => None,

            (Agent::Kilocode, Scope::Global) => home(".kilocode/rules/gw.md"),
            (Agent::Kilocode, Scope::Local) => Some(PathBuf::from(".kilocode/rules/gw.md")),

            (Agent::Antigravity, Scope::Local) => Some(PathBuf::from("AGENTS.md")),
            (Agent::Antigravity, Scope::Global) => None,

            (Agent::Opencode, Scope::Global) => home(".config/opencode/plugin/gw.ts"),
            (Agent::Opencode, Scope::Local) => Some(PathBuf::from(".opencode/plugin/gw.ts")),

            (Agent::Copilot, Scope::Local) => {
                Some(PathBuf::from(".github/copilot-instructions.md"))
            }
            (Agent::Copilot, Scope::Global) => None,
        }
    }

    /// Companion documentation file for hook-based agents — short note
    /// explaining auto-interception so the agent surfaces it in chat context.
    ///
    /// For Claude Code we use the official `.claude/rules/` directory rather
    /// than appending a marker block to the user's `CLAUDE.md`. See
    /// https://code.claude.com/docs/en/memory — `~/.claude/rules/*.md` is
    /// loaded into every session (global), `.claude/rules/*.md` per project.
    /// Owning a dedicated file keeps CLAUDE.md untouched.
    pub fn docs_path(self, scope: Scope) -> Option<PathBuf> {
        match (self, scope) {
            (Agent::ClaudeCode, Scope::Global) => home(".claude/rules/gw.md"),
            (Agent::ClaudeCode, Scope::Local) => Some(PathBuf::from(".claude/rules/gw.md")),
            (Agent::GeminiCli, Scope::Global) => home(".gemini/GEMINI.md"),
            (Agent::GeminiCli, Scope::Local) => Some(PathBuf::from("GEMINI.md")),
            (Agent::Cursor, Scope::Local) => Some(PathBuf::from("AGENTS.md")),
            (Agent::Cursor, Scope::Global) => home(".cursor/AGENTS.md"),
            _ => None,
        }
    }

    /// Legacy companion path — earlier versions appended a marker block to
    /// `CLAUDE.md` directly. `install` strips that block on upgrade so the
    /// content doesn't appear twice.
    pub fn legacy_docs_path(self, scope: Scope) -> Option<PathBuf> {
        match (self, scope) {
            (Agent::ClaudeCode, Scope::Global) => home(".claude/CLAUDE.md"),
            (Agent::ClaudeCode, Scope::Local) => Some(PathBuf::from("CLAUDE.md")),
            _ => None,
        }
    }
}

fn home(suffix: &str) -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(suffix))
}

#[derive(Debug)]
pub enum InstallOutcome {
    Installed,
    AlreadyInstalled,
}

#[derive(Debug)]
pub enum UninstallOutcome {
    Removed,
    NotPresent,
    NoFile,
}

/// What `gw doctor` finds at a given (agent, scope) location.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    /// File at the integration path doesn't exist.
    NoFile,
    /// File exists but contains no gw integration.
    NotInstalled,
    /// File exists and contains a gw integration registered with the current
    /// hook command / marker block.
    Installed,
    /// Claude Code only — integration registered with the legacy
    /// `gw hook claude` command (early gw versions).  Still functional thanks
    /// to the dispatcher alias, but `gw init --claude-code` will rewrite it on
    /// next run.
    InstalledLegacy,
}
