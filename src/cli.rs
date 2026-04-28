use clap::Parser;

use crate::init::Agent;

#[derive(Debug, Parser)]
#[command(
    name = "gw",
    about = "Gradle output filter — strips noise, keeps errors/warnings/status, shows heartbeat",
    version,
    trailing_var_arg = true,
    allow_hyphen_values = true,
    arg_required_else_help = true
)]
pub struct Cli {
    /// Pass-through mode: do not filter
    #[arg(long)]
    pub full: bool,

    /// Disable heartbeat (no progress lines on stderr while build runs)
    #[arg(long)]
    pub no_heartbeat: bool,

    /// Do not write the full log file under ./build-logs/
    #[arg(long)]
    pub no_log: bool,

    /// Errors only, no heartbeat, no warnings summary
    #[arg(long)]
    pub quiet: bool,

    /// Stream warnings as they appear (default: count only, summary at end)
    #[arg(long)]
    pub warnings: bool,

    /// Subcommand or command to run (init, uninstall, doctor, hook, rewrite, gain, or any external command)
    #[arg(required = true)]
    pub args: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum Dispatch<'a> {
    Init { agents: Vec<Agent>, local: bool },
    Uninstall { agents: Vec<Agent>, local: bool },
    Hook(String),
    Rewrite(&'a [String]),
    Gain(&'a [String]),
    Doctor,
    External(&'a [String]),
}

pub fn dispatch<'a>(args: &'a [String]) -> Result<Dispatch<'a>, String> {
    match args.first().map(String::as_str) {
        Some("init") => parse_agent_op(&args[1..], "gw init")
            .map(|(agents, local)| Dispatch::Init { agents, local }),
        Some("uninstall") => parse_agent_op(&args[1..], "gw uninstall")
            .map(|(agents, local)| Dispatch::Uninstall { agents, local }),
        Some("hook") => match args.get(1).map(String::as_str) {
            Some(name) => Ok(Dispatch::Hook(name.to_string())),
            None => Err("gw hook: agent name required (claude-code, gemini-cli, cursor)".into()),
        },
        Some("rewrite") => Ok(Dispatch::Rewrite(&args[1..])),
        Some("gain") => Ok(Dispatch::Gain(&args[1..])),
        Some("doctor") => {
            if args.len() > 1 {
                Err("gw doctor: takes no arguments".into())
            } else {
                Ok(Dispatch::Doctor)
            }
        }
        _ => Ok(Dispatch::External(args)),
    }
}

/// Parse the shared flag set used by `init` and `uninstall`:
///   `[--claude-code] [--gemini] [--codex] [--cursor] [--cline] [--windsurf]
///    [--kilocode] [--antigravity] [--opencode] [--copilot]
///    [--agent <key>]... [--all] [--local]`
///
/// If no agent flag is given, defaults to Claude Code.
fn parse_agent_op(args: &[String], cmd: &str) -> Result<(Vec<Agent>, bool), String> {
    let mut agents: Vec<Agent> = Vec::new();
    let mut local = false;
    let mut iter = args.iter();
    while let Some(f) = iter.next() {
        match f.as_str() {
            "--local" => local = true,
            "--all" => {
                for a in Agent::ALL {
                    push_unique(&mut agents, *a);
                }
            }
            "--agent" => {
                let key = iter
                    .next()
                    .ok_or_else(|| format!("{cmd}: --agent requires a value"))?;
                let agent =
                    Agent::from_key(key).ok_or_else(|| format!("{cmd}: unknown agent: {key}"))?;
                push_unique(&mut agents, agent);
            }
            flag if flag.starts_with("--") => match Agent::from_key(&flag[2..]) {
                Some(a) => push_unique(&mut agents, a),
                None => return Err(format!("{cmd}: unknown flag: {flag}")),
            },
            other => return Err(format!("{cmd}: unexpected argument: {other}")),
        }
    }
    if agents.is_empty() {
        agents.push(Agent::ClaudeCode);
    }
    Ok((agents, local))
}

fn push_unique(v: &mut Vec<Agent>, a: Agent) {
    if !v.contains(&a) {
        v.push(a);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn init_default_is_claude_code_global() {
        let args = s(&["init"]);
        match dispatch(&args).unwrap() {
            Dispatch::Init { agents, local } => {
                assert_eq!(agents, vec![Agent::ClaudeCode]);
                assert!(!local);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn init_multi_target() {
        let args = s(&["init", "--gemini-cli", "--codex", "--local"]);
        match dispatch(&args).unwrap() {
            Dispatch::Init { agents, local } => {
                assert_eq!(agents, vec![Agent::GeminiCli, Agent::Codex]);
                assert!(local);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn init_agent_flag_canonical() {
        let args = s(&["init", "--agent", "cursor"]);
        match dispatch(&args).unwrap() {
            Dispatch::Init { agents, .. } => assert_eq!(agents, vec![Agent::Cursor]),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn init_all_expands() {
        let args = s(&["init", "--all"]);
        match dispatch(&args).unwrap() {
            Dispatch::Init { agents, .. } => assert_eq!(agents.len(), Agent::ALL.len()),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn init_dedupes() {
        let args = s(&["init", "--codex", "--agent", "codex"]);
        match dispatch(&args).unwrap() {
            Dispatch::Init { agents, .. } => assert_eq!(agents, vec![Agent::Codex]),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn unknown_flag_errors() {
        let args = s(&["init", "--nope"]);
        assert!(dispatch(&args).is_err());
    }

    #[test]
    fn uninstall_parses_same_flags() {
        let args = s(&["uninstall", "--cursor"]);
        match dispatch(&args).unwrap() {
            Dispatch::Uninstall { agents, local } => {
                assert_eq!(agents, vec![Agent::Cursor]);
                assert!(!local);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn hook_requires_name() {
        let args = s(&["hook"]);
        assert!(dispatch(&args).is_err());
    }

    #[test]
    fn doctor_takes_no_args() {
        match dispatch(&s(&["doctor"])).unwrap() {
            Dispatch::Doctor => {}
            other => panic!("unexpected: {other:?}"),
        }
        assert!(dispatch(&s(&["doctor", "--all"])).is_err());
    }

    #[test]
    fn hook_passes_name_through() {
        let args = s(&["hook", "gemini-cli"]);
        match dispatch(&args).unwrap() {
            Dispatch::Hook(name) => assert_eq!(name, "gemini-cli"),
            other => panic!("unexpected: {other:?}"),
        }
    }
}
