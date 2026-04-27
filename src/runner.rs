use anyhow::{anyhow, Context, Result};
use std::io::{BufRead, BufReader, Write};
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

    let child_pid = child.id();

    // Install Ctrl-C handler: forward SIGTERM to the child so Gradle can clean up.
    // The handler is set once per process; ignore "already set" errors that appear
    // in test suites where run() is called multiple times.
    #[cfg(unix)]
    {
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
        Some(Heartbeat::start(
            std::time::Duration::from_secs(secs),
            std::time::Duration::from_millis(500),
        ))
    } else {
        None
    };

    let mut processor = Processor::new(opts.mode);
    let stdout_h = std::io::stdout();

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
        }
        if matches!(decision, Decision::Forward) {
            lines_forwarded += 1;
            bytes_forwarded += line.len() as u64 + 1;
            // Lock per-line so the heartbeat thread can interleave stdout
            // writes; pre-locking the handle would deadlock the heartbeat.
            {
                let mut out = stdout_h.lock();
                let _ = writeln!(out, "{}", line);
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

    summary::print(
        &processor.stats,
        log_path.as_deref(),
        &mut std::io::stderr().lock(),
    )?;

    let code = exit_code(status);
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
        if tx.send(line).is_err() {
            break;
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
