use std::io::Write as _;
use std::path::PathBuf;
use std::process::Command;

#[cfg(unix)]
extern crate libc;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_gw"))
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn run_gw(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(bin())
        .args(args)
        .env_remove("HOME")
        .output()
        .expect("spawn gw");
    let code = out.status.code().unwrap_or(-1);
    (
        code,
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn filters_successful_build() {
    let path = fixture("sample-success.txt");
    let (code, stdout, stderr) =
        run_gw(&["--no-log", "--no-heartbeat", "cat", path.to_str().unwrap()]);
    assert_eq!(code, 0);

    assert!(
        stdout.contains("BUILD SUCCESSFUL"),
        "missing build success: {stdout}"
    );
    assert!(
        !stdout.contains("Starting a Gradle Daemon"),
        "daemon noise leaked"
    );
    assert!(!stdout.contains("Download "), "download noise leaked");
    assert!(!stdout.contains("> Task :app:preBuild"), "task line leaked");
    assert!(!stdout.contains("Configure project"), "configure leaked");
    assert!(
        !stdout.contains("Parameter 'unused'"),
        "warning leaked by default"
    );

    assert!(
        stderr.contains("BUILD SUCCESSFUL"),
        "summary missing on stderr: {stderr}"
    );
    assert!(
        stderr.contains("warnings"),
        "no warning count in summary: {stderr}"
    );
}

#[test]
fn filters_failed_build() {
    let path = fixture("sample-failure.txt");
    let (_, stdout, _) = run_gw(&["--no-log", "--no-heartbeat", "cat", path.to_str().unwrap()]);

    assert!(
        stdout.contains("Unresolved reference: doStuff"),
        "kotlin error not forwarded"
    );
    assert!(
        stdout.contains("Unresolved reference: bar"),
        "second kotlin error not forwarded"
    );
    assert!(
        stdout.contains("FAILURE: Build failed"),
        "failure block not forwarded"
    );
    assert!(
        stdout.contains("Execution failed for task ':app:compileDebugKotlin'"),
        "failure detail missing"
    );
    assert!(stdout.contains("BUILD FAILED"), "build failed line missing");
    assert!(
        !stdout.contains("Starting a Gradle Daemon"),
        "daemon noise leaked"
    );
}

#[test]
fn rewrite_subcommand_handles_gradle_invocations() {
    let (code, stdout, _) = run_gw(&["rewrite", "./gradlew", "assemble"]);
    assert_eq!(code, 0);
    assert_eq!(stdout.trim(), "gw ./gradlew assemble");

    let (code, _, _) = run_gw(&["rewrite", "git", "status"]);
    assert_eq!(code, 1);

    let (code, stdout, _) = run_gw(&["rewrite", "./mainframer", "./gradlew", "test"]);
    assert_eq!(code, 0);
    assert_eq!(stdout.trim(), "gw ./mainframer ./gradlew test");
}

#[test]
fn hook_claude_returns_updated_input_on_match() {
    use std::io::Write;
    let mut child = Command::new(bin())
        .args(["hook", "claude"])
        .env_remove("HOME")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn hook");
    let payload = r#"{"tool_name":"Bash","tool_input":{"command":"./gradlew assembleDebug"}}"#;
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("wait");
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8(out.stdout).unwrap();
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("hook output is JSON");
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PreToolUse");
    assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "allow");
    assert_eq!(
        v["hookSpecificOutput"]["updatedInput"]["command"],
        "gw ./gradlew assembleDebug"
    );
}

#[test]
fn hook_claude_silent_for_non_gradle() {
    let mut child = Command::new(bin())
        .args(["hook", "claude"])
        .env_remove("HOME")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn hook");
    let payload = r#"{"tool_name":"Bash","tool_input":{"command":"git status"}}"#;
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("wait");
    assert_eq!(out.status.code(), Some(0));
    assert!(
        out.stdout.is_empty(),
        "expected no stdout, got {:?}",
        out.stdout
    );
}

/// H4: non-UTF8 bytes on stdin must exit 0 (not block the tool with non-zero).
#[test]
fn hook_claude_non_utf8_stdin_exits_zero() {
    let mut child = Command::new(bin())
        .args(["hook", "claude"])
        .env_remove("HOME")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn hook");
    // Feed raw non-UTF8 bytes followed by some ASCII.
    let bad_bytes: &[u8] = &[0xFF, 0xFE, 0x00, b'h', b'i'];
    child.stdin.as_mut().unwrap().write_all(bad_bytes).unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("wait");
    assert_eq!(
        out.status.code(),
        Some(0),
        "non-UTF8 input must not cause non-zero exit"
    );
}

/// C1 (Unix only): sending SIGINT to the gw process must propagate SIGTERM to
/// the child.  We verify this by checking that the child `sleep` process exits
/// shortly after we send SIGINT to gw.
#[cfg(unix)]
#[test]
fn sigint_forwarded_to_child() {
    use std::time::{Duration, Instant};

    // Spawn gw running a long sleep — if SIGTERM is forwarded the sleep exits early.
    let mut child = Command::new(bin())
        .args(["--no-log", "--no-heartbeat", "sleep", "30"])
        .env_remove("HOME")
        .spawn()
        .expect("spawn gw");

    let gw_pid = child.id();

    // Give gw a moment to spawn `sleep 30` and install the signal handler.
    std::thread::sleep(Duration::from_millis(300));

    // Locate the sleep child spawned by gw.
    let sleep_pid = {
        let output = Command::new("pgrep")
            .args(["-P", &gw_pid.to_string()])
            .output()
            .ok();
        output
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| s.trim().lines().next().map(|l| l.trim().to_string()))
            .and_then(|s| s.parse::<u32>().ok())
    };

    // Send SIGINT to the gw process (simulates Ctrl-C).
    unsafe {
        libc::kill(gw_pid as libc::pid_t, libc::SIGINT);
    }

    // Wait up to 4s for gw to exit.
    let deadline = Instant::now() + Duration::from_secs(4);
    let mut gw_exited = false;
    while Instant::now() < deadline {
        if let Ok(Some(_)) = child.try_wait() {
            gw_exited = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    // Always reap gw to avoid zombies.
    let _ = child.kill();
    let _ = child.wait();

    assert!(
        gw_exited,
        "gw did not exit within 4s after receiving SIGINT"
    );

    // If we could find the sleep PID, verify it is gone (no longer running).
    if let Some(pid) = sleep_pid {
        // Give the child a short moment to finish dying.
        std::thread::sleep(Duration::from_millis(500));
        let still_alive = unsafe { libc::kill(pid as libc::pid_t, 0) == 0 };
        assert!(
            !still_alive,
            "child sleep process (pid {pid}) is still alive — SIGTERM was not forwarded"
        );
    }
}
