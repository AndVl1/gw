pub mod consts;
pub mod settings;

use anyhow::{anyhow, Result};

#[derive(Debug, Clone, Copy)]
pub enum Scope {
    Global,
    Local,
}

pub fn install(scope: Scope) -> Result<()> {
    let path = match scope {
        Scope::Global => settings::global_path()
            .ok_or_else(|| anyhow!("HOME not set; cannot locate ~/.claude/settings.json"))?,
        Scope::Local => settings::local_path(),
    };
    match settings::install(&path)? {
        settings::InstallOutcome::Installed => {
            println!("Installed gw hook in {}", path.display());
            println!("Backup (if existed): {}.bak", path.display());
        }
        settings::InstallOutcome::AlreadyInstalled => {
            println!("gw hook already present in {}", path.display());
        }
    }
    Ok(())
}

pub fn uninstall(scope: Scope) -> Result<()> {
    let path = match scope {
        Scope::Global => settings::global_path()
            .ok_or_else(|| anyhow!("HOME not set; cannot locate ~/.claude/settings.json"))?,
        Scope::Local => settings::local_path(),
    };
    match settings::uninstall(&path)? {
        settings::UninstallOutcome::Removed => {
            println!("Removed gw hook from {}", path.display())
        }
        settings::UninstallOutcome::NotPresent => {
            println!("gw hook not present in {}", path.display())
        }
        settings::UninstallOutcome::NoFile => {
            println!("No settings file at {}", path.display())
        }
    }
    Ok(())
}
