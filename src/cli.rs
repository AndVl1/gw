use clap::Parser;

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

    /// Subcommand or command to run (init, hook, rewrite, or any external command)
    #[arg(required = true)]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
pub enum Dispatch<'a> {
    Init { local: bool, uninstall: bool },
    HookClaude,
    Rewrite(&'a [String]),
    Gain(&'a [String]),
    External(&'a [String]),
}

pub fn dispatch<'a>(args: &'a [String]) -> Result<Dispatch<'a>, String> {
    match args.first().map(String::as_str) {
        Some("init") => {
            let mut local = false;
            let mut uninstall = false;
            for f in &args[1..] {
                match f.as_str() {
                    "--local" => local = true,
                    "--uninstall" => uninstall = true,
                    flag if flag.starts_with("--") => {
                        return Err(format!("gw init: unknown flag: {flag}"));
                    }
                    _ => {}
                }
            }
            Ok(Dispatch::Init { local, uninstall })
        }
        Some("hook") => Ok(Dispatch::HookClaude),
        Some("rewrite") => Ok(Dispatch::Rewrite(&args[1..])),
        Some("gain") => Ok(Dispatch::Gain(&args[1..])),
        _ => Ok(Dispatch::External(args)),
    }
}
