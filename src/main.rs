mod cli;
mod filter;
mod gain;
mod heartbeat;
mod hook;
mod init;
mod log_writer;
mod parser;
mod runner;
mod stats;
mod summary;

use anyhow::Result;
use clap::Parser;

use crate::cli::{Cli, Dispatch};
use crate::filter::Mode;
use crate::runner::RunOptions;

fn main() {
    let exit_code = match real_main() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("gw: {e:#}");
            1
        }
    };
    std::process::exit(exit_code);
}

fn real_main() -> Result<i32> {
    let cli = Cli::parse();
    match cli::dispatch(&cli.args) {
        Err(msg) => {
            eprintln!("{msg}");
            Ok(2)
        }
        Ok(Dispatch::Init { local, uninstall }) => {
            let scope = if local {
                init::Scope::Local
            } else {
                init::Scope::Global
            };
            if uninstall {
                init::uninstall(scope)?;
            } else {
                init::install(scope)?;
            }
            Ok(0)
        }
        Ok(Dispatch::HookClaude) => hook::run_claude_hook(),
        Ok(Dispatch::Gain(rest)) => match gain::parse_args(rest) {
            Ok(opts) => gain::run(opts),
            Err(msg) => {
                eprintln!("{msg}");
                Ok(2)
            }
        },
        Ok(Dispatch::Rewrite(rest)) => {
            if rest.is_empty() {
                eprintln!("gw rewrite: command required");
                return Ok(2);
            }
            let joined = rest.join(" ");
            match hook::detect_rewrite(&joined) {
                Some(r) => {
                    println!("{r}");
                    Ok(0)
                }
                None => Ok(1),
            }
        }
        Ok(Dispatch::External(cmd)) => {
            if cmd.is_empty() {
                eprintln!("gw: no command provided");
                return Ok(2);
            }
            let opts = RunOptions {
                passthrough: cli.full,
                heartbeat: !cli.no_heartbeat && !cli.full,
                write_log: !cli.no_log && !cli.full,
                log_dir: None,
                mode: if cli.quiet {
                    Mode::Quiet
                } else if cli.warnings {
                    Mode::WithWarnings
                } else {
                    Mode::Default
                },
            };
            runner::run(cmd, opts)
        }
    }
}
