use anyhow::{anyhow, Context, Result};
use std::io::{BufRead, BufReader, IsTerminal, Write};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Instant;

use crate::filter::{Decision, Mode, Processor};
use crate::heartbeat::Heartbeat;
use crate::log_writer::LogWriter;
use crate::stats;
use crate::summary;

pub struct RunOptions {
    pub passthrough: bool,
    pub heartbeat: bool,
    pub write_log: bool,
    pub log_dir: Option<String>,
    pub mode: Mode,
}

/// Inject `--console=plain` into a command line if it looks like a Gradle
/// invocation and the user hasn't already specified a `--console=` value.
///
/// Why: wrapper commands (mainframer, ssh, custom shells) don't allocate a
/// real PTY for the remote process, but Gradle's auto-detection still
/// sometimes emits rich/ANSI output anyway. The escape sequences corrupt
/// our line-based parser (`BUILD FAILED` ends up after a cursor-move
/// sequence and the anchored regex misses it) and break terminal layout.
/// Forcing plain console makes the output deterministic.
///
/// Detection looks at the basename of each argument: matches when one of
/// them is `gradle`, `gradlew`, or `gradle.bat` (with any path prefix).
/// The flag is inserted right after the matched argument so it lands on
/// the gradle command itself, not on an outer wrapper.
pub fn inject_console_plain(args: &[String]) -> Vec<String> {
    let mut out: Vec<String> = args.to_vec();
    if out.iter().any(|a| has_console_flag(a)) {
        return out;
    }
    if let Some(idx) = out.iter().position(|a| is_gradle_invocation(a)) {
        out.insert(idx + 1, "--console=plain".to_string());
    }
    out
}

fn is_gradle_invocation(arg: &str) -> bool {
    let trimmed = arg.trim_end_matches('/');
    let basename = trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed);
    matches!(
        basename,
        "gradle" | "gradlew" | "gradle.bat" | "gradlew.bat"
    )
}

fn has_console_flag(arg: &str) -> bool {
    arg == "--console" || arg.starts_with("--console=")
}

pub fn run(args: &[String], opts: RunOptions) -> Result<i32> {
    if args.is_empty() {
        return Err(anyhow!("no command provided"));
    }
    let started = Instant::now();
    if opts.passthrough {
        let code = run_passthrough(args)?;
        record_passthrough(args, started, code);
        return Ok(code);
    }

    let mut log = if opts.write_log {
        Some(LogWriter::create(opts.log_dir.as_deref())?)
    } else {
        None
    };

    let mut child = Command::new(&args[0])
        .args(&args[1..])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to spawn {:?}", args))?;

    // Install Ctrl-C handler: forward SIGTERM to the child so Gradle can clean up.
    // The handler is set once per process; ignore "already set" errors that appear
    // in test suites where run() is called multiple times.
    #[cfg(unix)]
    {
        let child_pid = child.id();
        let _ = ctrlc::set_handler(move || {
            send_sigterm(child_pid);
        });
    }
    #[cfg(not(unix))]
    {
        // On non-Unix we cannot forward signals; install a no-op handler so
        // ctrlc's default (immediate exit) is at least replaced with a clean shutdown.
        let _ = ctrlc::set_handler(move || {});
    }

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    let (tx, rx) = mpsc::channel::<String>();
    let tx_err = tx.clone();

    let h_out = thread::spawn(move || pipe_lines(stdout, tx));
    let h_err = thread::spawn(move || pipe_lines(stderr, tx_err));

    let heartbeat = if opts.heartbeat {
        let secs = std::env::var("GW_HEARTBEAT_SILENT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(30);
        let slow_secs = std::env::var("GW_HEARTBEAT_SLOW_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(60);
        Some(Heartbeat::start(
            std::time::Duration::from_secs(secs),
            std::time::Duration::from_millis(500),
            std::time::Duration::from_secs(slow_secs),
        ))
    } else {
        None
    };

    let mut processor = Processor::new(opts.mode);
    let stdout_h = std::io::stdout();
    // CRLF only when stdout is a terminal — pipes/files keep plain LF so
    // downstream tools (grep, jq, files committed to git) aren't littered
    // with stray carriage returns.
    let line_ending: &[u8] = if stdout_h.is_terminal() {
        b"\r\n"
    } else {
        b"\n"
    };

    // Up-front disclaimer so the user (or a pasted log) immediately sees that
    // the stream is filtered, not the raw build. Printed once, before any
    // child output reaches stdout. Second line nudges automation agents not
    // to truncate already-trimmed output with `| tail -n N`.
    {
        let banner = banner_line(opts.mode, log.as_ref().map(|l| l.path()));
        let mut out = stdout_h.lock();
        let _ = out.write_all(banner.as_bytes());
        let _ = out.write_all(line_ending);
        let _ = out.write_all(BANNER_NOTRUNCATE.as_bytes());
        let _ = out.write_all(line_ending);
        let _ = out.flush();
    }

    // Track whether we already warned about a failing log write so we don't
    // spam the user on every subsequent line.
    let mut log_failed = false;

    let mut lines_in: u64 = 0;
    let mut lines_forwarded: u64 = 0;
    let mut bytes_in: u64 = 0;
    let mut bytes_forwarded: u64 = 0;

    while let Ok(line) = rx.recv() {
        lines_in += 1;
        bytes_in += line.len() as u64 + 1;
        if let Some(l) = log.as_mut() {
            if let Err(e) = l.write_line(&line) {
                if !log_failed {
                    eprintln!("gw: log file write failed: {e}; continuing without log");
                    log_failed = true;
                }
                log = None;
            }
        }
        let decision = processor.process(&line);
        if let Some(hb) = &heartbeat {
            hb.set_task(processor.current_task.clone());
            hb.set_progress(processor.progress_count);
        }
        if matches!(decision, Decision::Forward) {
            lines_forwarded += 1;
            bytes_forwarded += line.len() as u64 + 1;
            // Lock per-line so the heartbeat thread can interleave stdout
            // writes; pre-locking the handle would deadlock the heartbeat.
            //
            // Use explicit CRLF: child wrappers (ssh, mainframer) sometimes
            // leave the controlling tty in raw/-onlcr mode, so a bare `\n`
            // moves cursor down without returning to column 0 — output then
            // cascades right with each forwarded line. Writing `\r\n` is
            // safe in cooked mode too (ONLCR translates `\n` only and won't
            // double the CR).
            {
                let mut out = stdout_h.lock();
                let _ = out.write_all(line.as_bytes());
                let _ = out.write_all(line_ending);
                let _ = out.flush();
            }
            if let Some(hb) = &heartbeat {
                hb.note_output();
            }
        }
    }

    let _ = h_out.join();
    let _ = h_err.join();

    let status = child.wait().context("waiting for child failed")?;

    if let Some(hb) = heartbeat {
        hb.stop();
    }

    let log_path = log.as_ref().map(|l| l.path().to_path_buf());
    drop(log);

    let code = exit_code(status);
    summary::print(
        &processor.stats,
        log_path.as_deref(),
        Some(code),
        &mut std::io::stderr().lock(),
    )?;

    let run = stats::Run {
        ts: chrono::Utc::now().timestamp(),
        cmd: cmd_label(args),
        duration_ms: started.elapsed().as_millis() as u64,
        lines_in,
        lines_forwarded,
        bytes_in,
        bytes_forwarded,
        errors: processor.stats.errors,
        warnings: processor.stats.warnings,
        deprecations: processor.stats.deprecations,
        tasks_executed: processor.stats.tasks_executed,
        tasks_up_to_date: processor.stats.tasks_up_to_date,
        tasks_from_cache: processor.stats.tasks_from_cache,
        tests_passed: processor.stats.tests_passed,
        tests_failed: processor.stats.tests_failed,
        tests_skipped: processor.stats.tests_skipped,
        build_success: processor.stats.build_success,
        build_failed: processor.stats.build_failed,
        exit_code: code,
    };
    let _ = stats::append(&run);

    Ok(code)
}

fn record_passthrough(args: &[String], started: Instant, code: i32) {
    let run = stats::Run {
        ts: chrono::Utc::now().timestamp(),
        cmd: cmd_label(args),
        duration_ms: started.elapsed().as_millis() as u64,
        lines_in: 0,
        lines_forwarded: 0,
        bytes_in: 0,
        bytes_forwarded: 0,
        errors: 0,
        warnings: 0,
        deprecations: 0,
        tasks_executed: 0,
        tasks_up_to_date: 0,
        tasks_from_cache: 0,
        tests_passed: 0,
        tests_failed: 0,
        tests_skipped: 0,
        build_success: code == 0,
        build_failed: code != 0,
        exit_code: code,
    };
    let _ = stats::append(&run);
}

const BANNER_NOTRUNCATE: &str =
    "▸ gw: output is pre-filtered — do not pipe through tail/head, you will lose signal";

fn banner_line(mode: Mode, log_path: Option<&std::path::Path>) -> String {
    let forwarded = match mode {
        Mode::Quiet => "errors only",
        Mode::Default => "errors and build status",
        Mode::WithWarnings => "errors, warnings, and build status",
    };
    let mut s = format!("▸ gw: build starting — forwarding {forwarded}");
    if let Some(p) = log_path {
        s.push_str(&format!("; full log at {}", p.display()));
    }
    s
}

fn cmd_label(args: &[String]) -> String {
    args.iter().take(5).cloned().collect::<Vec<_>>().join(" ")
}

/// Send SIGTERM to a process by PID.  Called from the Ctrl-C handler.
#[cfg(unix)]
fn send_sigterm(pid: u32) {
    // Use the `kill` syscall directly via libc to avoid spawning a subprocess
    // from a signal handler context.
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGTERM);
    }
}

fn pipe_lines<R: std::io::Read + Send + 'static>(reader: R, tx: mpsc::Sender<String>) {
    let buf = BufReader::new(reader);
    for line in buf.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        // Some wrappers (ssh, mainframer, gradle progress bars) emit `\r`
        // mid-line to overwrite a previous status. BufReader::lines only
        // splits on `\n`, so those carriage returns end up embedded in the
        // forwarded line — corrupting terminal output and bypassing
        // anchored regexes like `^BUILD FAILED`. Treat each `\r`-separated
        // segment as its own logical line.
        for segment in line.split('\r') {
            if tx.send(segment.to_string()).is_err() {
                return;
            }
        }
    }
}

fn run_passthrough(args: &[String]) -> Result<i32> {
    let status = Command::new(&args[0])
        .args(&args[1..])
        .status()
        .with_context(|| format!("failed to spawn {:?}", args))?;
    Ok(exit_code(status))
}

fn exit_code(status: ExitStatus) -> i32 {
    status.code().unwrap_or_else(|| {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            128 + status.signal().unwrap_or(0)
        }
        #[cfg(not(unix))]
        {
            1
        }
    })
}

#[cfg(test)]
mod banner_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn default_mode_mentions_errors_and_status() {
        let b = banner_line(Mode::Default, None);
        assert!(b.starts_with("▸ gw: build starting"), "{b}");
        assert!(b.contains("errors and build status"), "{b}");
        assert!(!b.contains("full log"), "{b}");
    }

    #[test]
    fn with_warnings_mentions_warnings() {
        let b = banner_line(Mode::WithWarnings, None);
        assert!(b.contains("warnings"), "{b}");
    }

    #[test]
    fn quiet_mentions_errors_only() {
        let b = banner_line(Mode::Quiet, None);
        assert!(b.contains("errors only"), "{b}");
    }

    #[test]
    fn appends_log_path_when_present() {
        let b = banner_line(Mode::Default, Some(Path::new("build-logs/gw-x.log")));
        assert!(b.contains("full log at build-logs/gw-x.log"), "{b}");
    }
}

#[cfg(test)]
mod inject_console_plain_tests {
    use super::*;

    fn s(args: &[&str]) -> Vec<String> {
        args.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn injects_after_plain_gradlew() {
        let out = inject_console_plain(&s(&["./gradlew", "assemble"]));
        assert_eq!(out, s(&["./gradlew", "--console=plain", "assemble"]));
    }

    #[test]
    fn injects_after_gradlew_in_wrapper_chain() {
        let out = inject_console_plain(&s(&["./mainframer.sh", "./gradlew", "build"]));
        assert_eq!(
            out,
            s(&["./mainframer.sh", "./gradlew", "--console=plain", "build"])
        );
    }

    #[test]
    fn injects_with_absolute_path() {
        let out = inject_console_plain(&s(&["/usr/local/bin/gradle", "test"]));
        assert_eq!(
            out,
            s(&["/usr/local/bin/gradle", "--console=plain", "test"])
        );
    }

    #[test]
    fn windows_basename_matched() {
        let out = inject_console_plain(&s(&["C:\\proj\\gradlew.bat", "build"]));
        assert_eq!(
            out,
            s(&["C:\\proj\\gradlew.bat", "--console=plain", "build"])
        );
    }

    #[test]
    fn skips_when_user_set_console_flag() {
        let out = inject_console_plain(&s(&["./gradlew", "--console=rich", "assemble"]));
        assert_eq!(out, s(&["./gradlew", "--console=rich", "assemble"]));
    }

    #[test]
    fn skips_when_user_set_console_flag_with_space() {
        // Gradle accepts both `--console=plain` and `--console plain`.
        let out = inject_console_plain(&s(&["./gradlew", "--console", "plain", "assemble"]));
        assert_eq!(out, s(&["./gradlew", "--console", "plain", "assemble"]));
    }

    #[test]
    fn no_op_for_non_gradle_command() {
        let out = inject_console_plain(&s(&["cargo", "build"]));
        assert_eq!(out, s(&["cargo", "build"]));
    }

    #[test]
    fn no_op_for_arg_containing_gradle_substring() {
        // "my-gradle-helper" should NOT match — only a basename of `gradle`
        // (no suffix) is recognised.
        let out = inject_console_plain(&s(&["./my-gradle-helper", "go"]));
        assert_eq!(out, s(&["./my-gradle-helper", "go"]));
    }
}
