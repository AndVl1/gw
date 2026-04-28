//! `gw doctor` — read-only audit of every agent integration.
//!
//! Walks `Agent::ALL × Scope::{Global, Local}` and prints a status line for
//! each pair so the user can see at a glance which integrations are wired up,
//! which still rely on the legacy `gw hook claude` command, and which simply
//! have no file at the expected path.

use anyhow::Result;
use std::io::Write;

use crate::init::{self, Agent, AgentStatus, Scope};

pub fn run() -> Result<i32> {
    let mut out = std::io::stdout().lock();
    write_report(&mut out)
}

fn write_report<W: Write>(out: &mut W) -> Result<i32> {
    writeln!(out, "gw {}", env!("CARGO_PKG_VERSION"))?;
    writeln!(out)?;

    let mut any_legacy = false;

    for agent in Agent::ALL {
        writeln!(out, "{}:", agent.display())?;
        for scope in [Scope::Global, Scope::Local] {
            let label = scope_label(scope);
            match init::status_at(*agent, scope)? {
                None => writeln!(out, "  - {label:<6} unsupported")?,
                Some(status) => {
                    let path = agent
                        .path(scope)
                        .map(|p| p.display().to_string())
                        .unwrap_or_default();
                    let (glyph, text) = render(status);
                    writeln!(out, "  {glyph} {label:<6} {text:<14} {path}")?;
                    if matches!(status, AgentStatus::InstalledLegacy) {
                        any_legacy = true;
                    }
                }
            }
        }
    }

    if any_legacy {
        writeln!(out)?;
        writeln!(
            out,
            "note: legacy `gw hook claude` command detected. Re-run `gw init --claude-code` to migrate."
        )?;
    }

    Ok(0)
}

fn scope_label(scope: Scope) -> &'static str {
    match scope {
        Scope::Global => "global",
        Scope::Local => "local",
    }
}

fn render(status: AgentStatus) -> (char, &'static str) {
    match status {
        AgentStatus::Installed => ('✓', "installed"),
        AgentStatus::InstalledLegacy => ('⚠', "legacy"),
        AgentStatus::NotInstalled => ('✗', "not installed"),
        AgentStatus::NoFile => ('-', "no file"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_glyphs_cover_all_states() {
        assert_eq!(render(AgentStatus::Installed).0, '✓');
        assert_eq!(render(AgentStatus::InstalledLegacy).0, '⚠');
        assert_eq!(render(AgentStatus::NotInstalled).0, '✗');
        assert_eq!(render(AgentStatus::NoFile).0, '-');
    }

    #[test]
    fn report_includes_version_and_every_agent() {
        let mut buf: Vec<u8> = Vec::new();
        write_report(&mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains(&format!("gw {}", env!("CARGO_PKG_VERSION"))));
        for a in Agent::ALL {
            assert!(s.contains(a.display()), "missing {}", a.display());
        }
    }
}
