//! `gw upgrade` — re-run install for every (agent, scope) pair that already
//! has gw wired up, migrating the on-disk layout to the current scheme.
//!
//! Idempotent: if nothing about the install changed, it's a no-op (no backups,
//! no writes — `rules::install` returns `AlreadyInstalled` and JSON hooks
//! detect their own block by matcher/command). Anything stale (legacy
//! `gw hook claude` command, marker block in CLAUDE.md instead of
//! `.claude/rules/gw.md`, etc.) gets rewritten.
//!
//! Targets where status is `NotInstalled` or `NoFile` are skipped — the user
//! never asked for gw there, no reason to install.

use anyhow::Result;
use std::io::Write;

use crate::init::{self, Agent, AgentStatus, Scope};

pub fn run() -> Result<i32> {
    let mut out = std::io::stdout().lock();
    let mut upgraded = 0u32;
    let mut skipped = 0u32;

    writeln!(out, "gw upgrade ({})", env!("CARGO_PKG_VERSION"))?;
    writeln!(out)?;

    for agent in Agent::ALL {
        for scope in [Scope::Global, Scope::Local] {
            let status = match init::status_at(*agent, scope)? {
                None => continue, // unsupported scope for this agent
                Some(s) => s,
            };
            match status {
                AgentStatus::Installed | AgentStatus::InstalledLegacy => {
                    writeln!(
                        out,
                        "→ {} ({}): re-applying install",
                        agent.display(),
                        scope_label(scope)
                    )?;
                    init::install(*agent, scope)?;
                    upgraded += 1;
                }
                AgentStatus::NotInstalled | AgentStatus::NoFile => {
                    skipped += 1;
                }
            }
        }
    }

    writeln!(out)?;
    writeln!(
        out,
        "done: {upgraded} re-applied, {skipped} skipped (not installed)"
    )?;
    Ok(0)
}

fn scope_label(scope: Scope) -> &'static str {
    match scope {
        Scope::Global => "global",
        Scope::Local => "local",
    }
}
