pub mod agent;
pub mod consts;
pub mod json_hook;
pub mod opencode;
pub mod rules;
pub mod settings;

use anyhow::Result;
use std::path::Path;

pub use agent::{Agent, AgentStatus, InstallOutcome, IntegrationKind, Scope, UninstallOutcome};
use consts::{RULE_BODY, RULE_BODY_HOOK_NOTE};

/// Install gw integration for the given agent at the given scope.
///
/// For hook-based agents, also writes a short companion docs note (CLAUDE.md /
/// GEMINI.md / AGENTS.md) so the agent surfaces "auto-intercepted via hooks"
/// in its instructions context.
pub fn install(agent: Agent, scope: Scope) -> Result<()> {
    let Some(path) = agent.path(scope) else {
        eprintln!(
            "gw: {} does not support {} scope — skipping",
            agent.display(),
            scope_label(scope)
        );
        return Ok(());
    };

    let outcome = install_at(agent, &path)?;
    print_install(agent, &path, &outcome);

    // Companion docs note for hook-based agents only.
    if let Some(docs_path) = agent.docs_path(scope) {
        match rules::install(&docs_path, RULE_BODY_HOOK_NOTE)? {
            InstallOutcome::Installed => {
                println!("  + docs note: {}", docs_path.display());
            }
            InstallOutcome::AlreadyInstalled => {}
        }
    }
    Ok(())
}

pub fn uninstall(agent: Agent, scope: Scope) -> Result<()> {
    let Some(path) = agent.path(scope) else {
        eprintln!(
            "gw: {} does not support {} scope — skipping",
            agent.display(),
            scope_label(scope)
        );
        return Ok(());
    };

    let outcome = uninstall_at(agent, &path)?;
    print_uninstall(agent, &path, &outcome);

    if let Some(docs_path) = agent.docs_path(scope) {
        match rules::uninstall(&docs_path)? {
            UninstallOutcome::Removed => {
                println!("  - docs note: {}", docs_path.display());
            }
            UninstallOutcome::NotPresent | UninstallOutcome::NoFile => {}
        }
    }
    Ok(())
}

/// Inspect the integration state for `(agent, scope)` without modifying anything.
///
/// Returns `Ok(None)` if the agent does not support the requested scope.
pub fn status_at(agent: Agent, scope: Scope) -> Result<Option<AgentStatus>> {
    let Some(path) = agent.path(scope) else {
        return Ok(None);
    };
    let status = match agent.kind() {
        IntegrationKind::ClaudeHook => json_hook::status_claude(&path)?,
        IntegrationKind::GeminiHook => json_hook::status_gemini(&path)?,
        IntegrationKind::CursorHook => json_hook::status_cursor(&path)?,
        IntegrationKind::OpencodePlugin => opencode::status(&path)?,
        IntegrationKind::RulesAppend => rules::status(&path)?,
    };
    Ok(Some(status))
}

fn install_at(agent: Agent, path: &Path) -> Result<InstallOutcome> {
    match agent.kind() {
        IntegrationKind::ClaudeHook => json_hook::install_claude(path),
        IntegrationKind::GeminiHook => json_hook::install_gemini(path),
        IntegrationKind::CursorHook => json_hook::install_cursor(path),
        IntegrationKind::OpencodePlugin => opencode::install(path),
        IntegrationKind::RulesAppend => rules::install(path, RULE_BODY),
    }
}

fn uninstall_at(agent: Agent, path: &Path) -> Result<UninstallOutcome> {
    match agent.kind() {
        IntegrationKind::ClaudeHook => json_hook::uninstall_claude(path),
        IntegrationKind::GeminiHook => json_hook::uninstall_gemini(path),
        IntegrationKind::CursorHook => json_hook::uninstall_cursor(path),
        IntegrationKind::OpencodePlugin => opencode::uninstall(path),
        IntegrationKind::RulesAppend => rules::uninstall(path),
    }
}

fn print_install(agent: Agent, path: &Path, outcome: &InstallOutcome) {
    match outcome {
        InstallOutcome::Installed => {
            println!("✓ {} installed → {}", agent.display(), path.display());
        }
        InstallOutcome::AlreadyInstalled => {
            println!(
                "• {} already installed at {}",
                agent.display(),
                path.display()
            );
        }
    }
}

fn print_uninstall(agent: Agent, path: &Path, outcome: &UninstallOutcome) {
    match outcome {
        UninstallOutcome::Removed => {
            println!("✓ {} removed from {}", agent.display(), path.display());
        }
        UninstallOutcome::NotPresent => {
            println!("• {} not present in {}", agent.display(), path.display());
        }
        UninstallOutcome::NoFile => {
            println!("• {} no file at {}", agent.display(), path.display());
        }
    }
}

fn scope_label(scope: Scope) -> &'static str {
    match scope {
        Scope::Global => "global",
        Scope::Local => "local",
    }
}
