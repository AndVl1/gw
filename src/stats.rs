use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

/// One record per `gw` invocation, appended as a JSONL line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    pub ts: i64,
    pub cmd: String,
    pub duration_ms: u64,
    pub lines_in: u64,
    pub lines_forwarded: u64,
    pub bytes_in: u64,
    pub bytes_forwarded: u64,
    pub errors: u32,
    pub warnings: u32,
    pub deprecations: u32,
    pub tasks_executed: u32,
    pub tasks_up_to_date: u32,
    pub tasks_from_cache: u32,
    pub tests_passed: u32,
    pub tests_failed: u32,
    pub tests_skipped: u32,
    pub build_success: bool,
    pub build_failed: bool,
    pub exit_code: i32,
}

pub fn store_path() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|h| h.join(".local").join("share").join("gw").join("runs.jsonl"))
}

/// Append a run record to the JSONL store. Best-effort: errors are returned but
/// callers typically ignore them so a failing stats file never breaks a build.
pub fn append(record: &Run) -> Result<()> {
    let Some(path) = store_path() else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create stats dir {}", parent.display()))?;
    }
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    let line = serde_json::to_string(record)?;
    f.write_all(line.as_bytes())?;
    f.write_all(b"\n")?;
    Ok(())
}

pub fn load_all() -> Result<Vec<Run>> {
    let Some(path) = store_path() else {
        return Ok(Vec::new());
    };
    if !path.exists() {
        return Ok(Vec::new());
    }
    let f = OpenOptions::new().read(true).open(&path)?;
    let buf = BufReader::new(f);
    let mut out = Vec::new();
    for line in buf.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(r) = serde_json::from_str::<Run>(&line) {
            out.push(r);
        }
    }
    Ok(out)
}

#[derive(Debug, Default)]
pub struct Aggregate {
    pub runs: u64,
    pub failed: u64,
    pub duration_ms_total: u64,
    pub lines_in: u64,
    pub lines_forwarded: u64,
    pub bytes_in: u64,
    pub bytes_forwarded: u64,
    pub errors: u64,
    pub warnings: u64,
}

pub fn aggregate(runs: &[Run]) -> Aggregate {
    let mut a = Aggregate::default();
    for r in runs {
        a.runs += 1;
        if r.build_failed || (!r.build_success && r.exit_code != 0) {
            a.failed += 1;
        }
        a.duration_ms_total += r.duration_ms;
        a.lines_in += r.lines_in;
        a.lines_forwarded += r.lines_forwarded;
        a.bytes_in += r.bytes_in;
        a.bytes_forwarded += r.bytes_forwarded;
        a.errors += r.errors as u64;
        a.warnings += r.warnings as u64;
    }
    a
}

/// Parse a duration like "7d", "12h", "30m", "90s" into seconds.
pub fn parse_duration(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (num, unit) = s.split_at(s.len().saturating_sub(1));
    let n: i64 = num.parse().ok()?;
    match unit {
        "s" => Some(n),
        "m" => Some(n * 60),
        "h" => Some(n * 3600),
        "d" => Some(n * 86400),
        "w" => Some(n * 86400 * 7),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_durations() {
        assert_eq!(parse_duration("30s"), Some(30));
        assert_eq!(parse_duration("5m"), Some(300));
        assert_eq!(parse_duration("2h"), Some(7200));
        assert_eq!(parse_duration("7d"), Some(7 * 86400));
        assert_eq!(parse_duration("1w"), Some(7 * 86400));
        assert_eq!(parse_duration(""), None);
        assert_eq!(parse_duration("foo"), None);
    }

    #[test]
    fn aggregate_sums_fields() {
        let runs = vec![
            Run {
                ts: 0,
                cmd: "x".into(),
                duration_ms: 1000,
                lines_in: 100,
                lines_forwarded: 5,
                bytes_in: 10_000,
                bytes_forwarded: 500,
                errors: 0,
                warnings: 3,
                deprecations: 0,
                tasks_executed: 0,
                tasks_up_to_date: 0,
                tasks_from_cache: 0,
                tests_passed: 0,
                tests_failed: 0,
                tests_skipped: 0,
                build_success: true,
                build_failed: false,
                exit_code: 0,
            },
            Run {
                ts: 0,
                cmd: "x".into(),
                duration_ms: 2000,
                lines_in: 200,
                lines_forwarded: 10,
                bytes_in: 20_000,
                bytes_forwarded: 1_000,
                errors: 2,
                warnings: 1,
                deprecations: 0,
                tasks_executed: 0,
                tasks_up_to_date: 0,
                tasks_from_cache: 0,
                tests_passed: 0,
                tests_failed: 0,
                tests_skipped: 0,
                build_success: false,
                build_failed: true,
                exit_code: 1,
            },
        ];
        let a = aggregate(&runs);
        assert_eq!(a.runs, 2);
        assert_eq!(a.failed, 1);
        assert_eq!(a.duration_ms_total, 3000);
        assert_eq!(a.lines_in, 300);
        assert_eq!(a.lines_forwarded, 15);
        assert_eq!(a.errors, 2);
    }
}
