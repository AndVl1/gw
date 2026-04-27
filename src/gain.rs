use anyhow::Result;
use std::io::Write;

use crate::stats::{self, aggregate, parse_duration, Run};

#[derive(Debug, Default, Clone, Copy)]
pub struct GainOptions {
    pub since_secs: Option<i64>,
    pub history: bool,
    pub limit: usize,
}

pub fn parse_args(args: &[String]) -> Result<GainOptions, String> {
    let mut opts = GainOptions {
        since_secs: None,
        history: false,
        limit: 10,
    };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--since" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "gw gain: --since requires a value (e.g. 7d)".to_string())?;
                opts.since_secs = Some(
                    parse_duration(v).ok_or_else(|| format!("gw gain: bad --since value: {v}"))?,
                );
                i += 2;
            }
            "--history" => {
                opts.history = true;
                i += 1;
            }
            "--limit" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "gw gain: --limit requires a number".to_string())?;
                opts.limit = v
                    .parse()
                    .map_err(|_| format!("gw gain: bad --limit value: {v}"))?;
                i += 2;
            }
            other => return Err(format!("gw gain: unknown argument: {other}")),
        }
    }
    Ok(opts)
}

pub fn run(opts: GainOptions) -> Result<i32> {
    let all = stats::load_all()?;
    let now = chrono::Utc::now().timestamp();
    let filtered: Vec<Run> = match opts.since_secs {
        Some(secs) => all.into_iter().filter(|r| now - r.ts <= secs).collect(),
        None => all,
    };

    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    if opts.history {
        print_history(&filtered, opts.limit, &mut out)?;
    } else {
        print_summary(&filtered, opts.since_secs, &mut out)?;
    }
    Ok(0)
}

fn print_summary<W: Write>(runs: &[Run], since_secs: Option<i64>, w: &mut W) -> Result<()> {
    let a = aggregate(runs);
    let header = match since_secs {
        Some(s) => format!(
            "gw stats ({} runs, last {})",
            a.runs,
            humanize_duration_secs(s)
        ),
        None => format!("gw stats ({} runs, all time)", a.runs),
    };
    writeln!(w, "{header}")?;
    writeln!(w, "─────────────────────────────────────")?;
    if a.runs == 0 {
        writeln!(w, "No runs recorded yet.")?;
        return Ok(());
    }
    let lines_pct = pct_suppressed(a.lines_in, a.lines_forwarded);
    let bytes_pct = pct_suppressed(a.bytes_in, a.bytes_forwarded);
    writeln!(
        w,
        "Lines:   in {} → out {}  ({:.1}% suppressed)",
        fmt_count(a.lines_in),
        fmt_count(a.lines_forwarded),
        lines_pct
    )?;
    writeln!(
        w,
        "Bytes:   in {} → out {}  ({:.1}% saved)",
        fmt_bytes(a.bytes_in),
        fmt_bytes(a.bytes_forwarded),
        bytes_pct
    )?;
    let avg = if a.runs > 0 {
        a.duration_ms_total / a.runs
    } else {
        0
    };
    writeln!(
        w,
        "Time:    avg {}/build, {} failed",
        fmt_duration_ms(avg),
        a.failed
    )?;
    writeln!(w, "Errors:  {}, warnings: {}", a.errors, a.warnings)?;
    Ok(())
}

fn print_history<W: Write>(runs: &[Run], limit: usize, w: &mut W) -> Result<()> {
    let n = runs.len();
    let start = n.saturating_sub(limit);
    let slice = &runs[start..];
    writeln!(
        w,
        "{:<19} {:>7} {:>9} {:>9} {:>5} cmd",
        "when", "dur", "lines", "out", "code"
    )?;
    for r in slice {
        let when = chrono::DateTime::<chrono::Utc>::from_timestamp(r.ts, 0)
            .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| r.ts.to_string());
        writeln!(
            w,
            "{:<19} {:>7} {:>9} {:>9} {:>5} {}",
            when,
            fmt_duration_ms(r.duration_ms),
            fmt_count(r.lines_in),
            fmt_count(r.lines_forwarded),
            r.exit_code,
            r.cmd
        )?;
    }
    Ok(())
}

fn pct_suppressed(input: u64, forwarded: u64) -> f64 {
    if input == 0 {
        0.0
    } else {
        100.0 * (input.saturating_sub(forwarded) as f64) / (input as f64)
    }
}

fn fmt_count(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

fn fmt_bytes(n: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if n >= GB {
        format!("{:.1}G", n as f64 / GB as f64)
    } else if n >= MB {
        format!("{:.1}M", n as f64 / MB as f64)
    } else if n >= KB {
        format!("{:.1}K", n as f64 / KB as f64)
    } else {
        format!("{n}B")
    }
}

fn fmt_duration_ms(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}h{}m", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("{}m{}s", s / 60, s % 60)
    } else if s >= 1 {
        format!("{}s", s)
    } else {
        format!("{}ms", ms)
    }
}

fn humanize_duration_secs(s: i64) -> String {
    if s % 86400 == 0 {
        format!("{}d", s / 86400)
    } else if s % 3600 == 0 {
        format!("{}h", s / 3600)
    } else if s % 60 == 0 {
        format!("{}m", s / 60)
    } else {
        format!("{}s", s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_count_groups_thousands() {
        assert_eq!(fmt_count(0), "0");
        assert_eq!(fmt_count(999), "999");
        assert_eq!(fmt_count(1_000), "1,000");
        assert_eq!(fmt_count(142_538), "142,538");
        assert_eq!(fmt_count(1_234_567), "1,234,567");
    }

    #[test]
    fn fmt_bytes_picks_unit() {
        assert_eq!(fmt_bytes(500), "500B");
        assert_eq!(fmt_bytes(2048), "2.0K");
        assert_eq!(fmt_bytes(2 * 1024 * 1024), "2.0M");
    }

    #[test]
    fn fmt_duration_ranges() {
        assert_eq!(fmt_duration_ms(500), "500ms");
        assert_eq!(fmt_duration_ms(1500), "1s");
        assert_eq!(fmt_duration_ms(84_000), "1m24s");
        assert_eq!(fmt_duration_ms(3_660_000), "1h1m");
    }

    #[test]
    fn parse_args_handles_flags() {
        let args = vec!["--since".into(), "7d".into()];
        let opts = parse_args(&args).unwrap();
        assert_eq!(opts.since_secs, Some(7 * 86400));
        assert!(!opts.history);

        let args = vec!["--history".into(), "--limit".into(), "5".into()];
        let opts = parse_args(&args).unwrap();
        assert!(opts.history);
        assert_eq!(opts.limit, 5);
    }

    #[test]
    fn parse_args_rejects_bad_input() {
        assert!(parse_args(&["--since".into()]).is_err());
        assert!(parse_args(&["--unknown".into()]).is_err());
        assert!(parse_args(&["--limit".into(), "abc".into()]).is_err());
    }
}
