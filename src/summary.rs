use crate::filter::Stats;
use std::io::{Result, Write};
use std::path::Path;

pub fn print<W: Write>(
    stats: &Stats,
    log_path: Option<&Path>,
    exit_code: Option<i32>,
    out: &mut W,
) -> Result<()> {
    writeln!(out, "─────────────────────────────────────")?;
    let status: String = if stats.build_failed {
        "BUILD FAILED".into()
    } else if stats.build_success {
        "BUILD SUCCESSFUL".into()
    } else {
        match exit_code {
            Some(0) => "BUILD SUCCESSFUL (no status line)".into(),
            Some(c) => format!("BUILD FAILED (exit {c})"),
            None => "BUILD ENDED (no status reported)".into(),
        }
    };
    writeln!(out, "{status}")?;

    let total_tasks = stats.tasks_executed
        + stats.tasks_up_to_date
        + stats.tasks_from_cache
        + stats.tasks_skipped
        + stats.tasks_no_source
        + stats.tasks_failed;
    if total_tasks > 0 {
        let mut parts = Vec::new();
        if stats.tasks_executed > 0 {
            parts.push(format!("{} executed", stats.tasks_executed));
        }
        if stats.tasks_up_to_date > 0 {
            parts.push(format!("{} up-to-date", stats.tasks_up_to_date));
        }
        if stats.tasks_from_cache > 0 {
            parts.push(format!("{} from-cache", stats.tasks_from_cache));
        }
        if stats.tasks_skipped > 0 {
            parts.push(format!("{} skipped", stats.tasks_skipped));
        }
        if stats.tasks_no_source > 0 {
            parts.push(format!("{} no-source", stats.tasks_no_source));
        }
        if stats.tasks_failed > 0 {
            parts.push(format!("{} FAILED", stats.tasks_failed));
        }
        writeln!(out, "Tasks: {}", parts.join(", "))?;
    }

    let total_tests = stats.tests_passed + stats.tests_failed + stats.tests_skipped;
    if total_tests > 0 {
        writeln!(
            out,
            "Tests: {} passed, {} failed, {} skipped",
            stats.tests_passed, stats.tests_failed, stats.tests_skipped
        )?;
    }

    if stats.errors > 0 || stats.warnings > 0 || stats.deprecations > 0 {
        writeln!(
            out,
            "Diagnostics: {} errors, {} warnings, {} deprecations",
            stats.errors, stats.warnings, stats.deprecations
        )?;
    }

    if let Some(p) = log_path {
        writeln!(out, "Full log: {}", p.display())?;
    }
    writeln!(out, "─────────────────────────────────────")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::Stats;

    #[test]
    fn renders_basic_summary() {
        let stats = Stats {
            build_success: true,
            tasks_executed: 5,
            tasks_up_to_date: 10,
            warnings: 2,
            ..Stats::default()
        };
        let mut buf = Vec::new();
        print(&stats, None, Some(0), &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("BUILD SUCCESSFUL"));
        assert!(s.contains("5 executed"));
        assert!(s.contains("10 up-to-date"));
        assert!(s.contains("2 warnings"));
    }

    #[test]
    fn falls_back_to_exit_zero_as_success() {
        let stats = Stats::default();
        let mut buf = Vec::new();
        print(&stats, None, Some(0), &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("BUILD SUCCESSFUL (no status line)"), "{s}");
    }

    #[test]
    fn falls_back_to_exit_nonzero_as_failed() {
        let stats = Stats::default();
        let mut buf = Vec::new();
        print(&stats, None, Some(1), &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("BUILD FAILED (exit 1)"), "{s}");
    }

    #[test]
    fn unknown_exit_keeps_legacy_message() {
        let stats = Stats::default();
        let mut buf = Vec::new();
        print(&stats, None, None, &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("BUILD ENDED (no status reported)"), "{s}");
    }
}
