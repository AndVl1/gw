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

/// End-to-end heartbeat smoke test. With a short silent threshold, gw should
/// emit at least one "▸ building" heartbeat line on stdout while the wrapped
/// command produces no output. Also verifies gw exits cleanly afterwards
/// (regression test for the stdout-lock deadlock between main and heartbeat
/// thread).
#[test]
fn heartbeat_fires_during_silent_command() {
    let out = Command::new(bin())
        .args(["--no-log", "sleep", "3"])
        .env_remove("HOME")
        .env("GW_HEARTBEAT_SILENT_SECS", "1")
        .output()
        .expect("spawn gw");

    let code = out.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert_eq!(code, 0, "gw should exit cleanly; stdout: {stdout}");
    assert!(
        stdout.contains("▸ building"),
        "expected at least one heartbeat line, got stdout: {stdout}"
    );
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
fn forwards_informational_task_listing() {
    let path = fixture("sample-tasks.txt");
    let (code, stdout, _) = run_gw(&["--no-log", "--no-heartbeat", "cat", path.to_str().unwrap()]);
    assert_eq!(code, 0);

    // Listing body must reach the user.
    assert!(
        stdout.contains("Build tasks"),
        "section header missing: {stdout}"
    );
    assert!(
        stdout.contains("assemble - Assembles the outputs of this project."),
        "task description missing: {stdout}"
    );
    assert!(
        stdout.contains("To see all tasks and more detail, run gradlew tasks --all"),
        "tail line missing: {stdout}"
    );
    assert!(stdout.contains("BUILD SUCCESSFUL"), "build line missing");

    // Configuration phase + daemon noise must still be filtered.
    assert!(
        !stdout.contains("Starting a Gradle Daemon"),
        "daemon noise leaked: {stdout}"
    );
    assert!(
        !stdout.contains("Configure project"),
        "configure phase leaked: {stdout}"
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

/// `gw hook gemini-cli` parses Gemini's tool_input shape and emits an
/// allow+rewrite envelope.
#[test]
fn hook_gemini_rewrites_gradle() {
    let mut child = Command::new(bin())
        .args(["hook", "gemini-cli"])
        .env_remove("HOME")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn hook");
    let payload = r#"{"tool_name":"bash","tool_input":{"command":"./gradlew test"}}"#;
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("wait");
    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("hook output is JSON");
    assert_eq!(v["decision"], "allow");
    assert_eq!(v["tool_input"]["command"], "gw ./gradlew test");
}

/// `gw hook cursor` denies and instructs the agent to retry — Cursor lacks
/// an in-band rewrite mechanism.
#[test]
fn hook_cursor_denies_with_retry_message() {
    let mut child = Command::new(bin())
        .args(["hook", "cursor"])
        .env_remove("HOME")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn hook");
    let payload = r#"{"command":"./gradlew assembleDebug","cwd":"/tmp","hook_event_name":"beforeShellExecution"}"#;
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("wait");
    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("hook output is JSON");
    assert_eq!(v["permission"], "deny");
    let msg = v["agentMessage"].as_str().unwrap();
    assert!(
        msg.contains("gw ./gradlew assembleDebug"),
        "agentMessage should embed rewritten command: {msg}"
    );
}

/// `gw hook <unknown>` exits non-zero with an error message.
#[test]
fn hook_unknown_agent_errors() {
    let (code, _, stderr) = run_gw(&["hook", "nope"]);
    assert_eq!(code, 2);
    assert!(stderr.contains("unknown agent"), "stderr: {stderr}");
}

/// `gw init --agent codex --local` writes AGENTS.md with marker block.
#[test]
fn init_codex_local_writes_agents_md() {
    use tempfile::tempdir;
    let dir = tempdir().unwrap();
    let out = Command::new(bin())
        .args(["init", "--agent", "codex", "--local"])
        .current_dir(dir.path())
        .env_remove("HOME")
        .output()
        .expect("spawn gw init");
    assert_eq!(out.status.code(), Some(0));
    let agents = dir.path().join("AGENTS.md");
    assert!(agents.exists());
    let content = std::fs::read_to_string(&agents).unwrap();
    assert!(content.contains("<!-- gw:begin -->"));
    assert!(content.contains("<!-- gw:end -->"));
    assert!(content.contains("Build Commands"));

    // Idempotent: second invocation should be a no-op (no extra block).
    let out2 = Command::new(bin())
        .args(["init", "--agent", "codex", "--local"])
        .current_dir(dir.path())
        .env_remove("HOME")
        .output()
        .expect("spawn gw init #2");
    assert_eq!(out2.status.code(), Some(0));
    let content2 = std::fs::read_to_string(&agents).unwrap();
    assert_eq!(content.matches("<!-- gw:begin -->").count(), 1);
    assert_eq!(content2.matches("<!-- gw:begin -->").count(), 1);

    // Uninstall removes the block.
    let out3 = Command::new(bin())
        .args(["uninstall", "--agent", "codex", "--local"])
        .current_dir(dir.path())
        .env_remove("HOME")
        .output()
        .expect("spawn gw uninstall");
    assert_eq!(out3.status.code(), Some(0));
    // File should be gone (only our block remained).
    assert!(!agents.exists());
}

/// `gw init --gemini-cli --local` writes Gemini settings with BeforeTool hook.
#[test]
fn init_gemini_local_writes_settings() {
    use tempfile::tempdir;
    let dir = tempdir().unwrap();
    let out = Command::new(bin())
        .args(["init", "--gemini-cli", "--local"])
        .current_dir(dir.path())
        .env_remove("HOME")
        .output()
        .expect("spawn gw init");
    assert_eq!(out.status.code(), Some(0));
    let settings = dir.path().join(".gemini/settings.json");
    assert!(settings.exists());
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    let arr = v["hooks"]["BeforeTool"].as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["command"], "gw hook gemini-cli");
    assert_eq!(arr[0]["matcher"]["tool"], "bash");

    // Companion docs note (GEMINI.md) created.
    let docs = dir.path().join("GEMINI.md");
    assert!(docs.exists());
    let docs_content = std::fs::read_to_string(&docs).unwrap();
    assert!(docs_content.contains("auto-intercepted"));
}

/// `gw init --cursor --local` writes Cursor hooks.json with beforeShellExecution.
#[test]
fn init_cursor_local_writes_hooks_json() {
    use tempfile::tempdir;
    let dir = tempdir().unwrap();
    let out = Command::new(bin())
        .args(["init", "--cursor", "--local"])
        .current_dir(dir.path())
        .env_remove("HOME")
        .output()
        .expect("spawn gw init");
    assert_eq!(out.status.code(), Some(0));
    let hooks = dir.path().join(".cursor/hooks.json");
    assert!(hooks.exists());
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&hooks).unwrap()).unwrap();
    assert_eq!(v["version"], 1);
    let arr = v["hooks"]["beforeShellExecution"].as_array().unwrap();
    assert_eq!(arr[0]["command"], "gw hook cursor");
}

/// Multi-target install in one invocation.
#[test]
fn init_multi_target_local() {
    use tempfile::tempdir;
    let dir = tempdir().unwrap();
    let out = Command::new(bin())
        .args(["init", "--codex", "--cline", "--local"])
        .current_dir(dir.path())
        .env_remove("HOME")
        .output()
        .expect("spawn gw init");
    assert_eq!(out.status.code(), Some(0));
    assert!(dir.path().join("AGENTS.md").exists());
    assert!(dir.path().join(".clinerules").exists());
}

/// `gw init --claude-code --local` writes hook into settings.local.json AND
/// drops the rule file at `.claude/rules/gw.md` (NOT into CLAUDE.md).
#[test]
fn init_claude_code_local_writes_rules_file_not_claude_md() {
    use tempfile::tempdir;
    let dir = tempdir().unwrap();
    let out = Command::new(bin())
        .args(["init", "--claude-code", "--local"])
        .current_dir(dir.path())
        .env_remove("HOME")
        .output()
        .expect("spawn gw init");
    assert_eq!(out.status.code(), Some(0));

    // Hook registered.
    let settings = dir.path().join(".claude/settings.local.json");
    assert!(settings.exists());

    // Rule file written at .claude/rules/gw.md (official Claude Code convention).
    let rules = dir.path().join(".claude/rules/gw.md");
    assert!(rules.exists(), "expected .claude/rules/gw.md to exist");
    let content = std::fs::read_to_string(&rules).unwrap();
    assert!(content.contains("auto-intercepted"));
    assert!(content.contains("<!-- gw:begin -->"));

    // CLAUDE.md must NOT be touched.
    assert!(
        !dir.path().join("CLAUDE.md").exists(),
        "CLAUDE.md must not be created — gw owns its own rules file"
    );
}

/// `gw init` after an older install that had written a marker block into
/// CLAUDE.md: the legacy block is stripped, the new rules/gw.md is created.
#[test]
fn init_claude_code_migrates_legacy_claude_md_block() {
    use tempfile::tempdir;
    let dir = tempdir().unwrap();
    let claude_md = dir.path().join("CLAUDE.md");
    std::fs::write(
        &claude_md,
        "# Project rules\n\nUse strict types.\n\n<!-- gw:begin -->\n## Build Commands (managed by gw)\n\nold body\n<!-- gw:end -->\n",
    )
    .unwrap();

    let out = Command::new(bin())
        .args(["init", "--claude-code", "--local"])
        .current_dir(dir.path())
        .env_remove("HOME")
        .output()
        .expect("spawn gw init");
    assert_eq!(out.status.code(), Some(0));

    // Legacy block stripped, user content preserved.
    let after = std::fs::read_to_string(&claude_md).unwrap();
    assert!(after.contains("Project rules"));
    assert!(!after.contains("<!-- gw:begin -->"));
    assert!(!after.contains("old body"));

    // New rules file in place.
    assert!(dir.path().join(".claude/rules/gw.md").exists());

    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("migrated legacy block") || stderr.contains("migrated legacy block"),
        "expected migration message, stdout: {stdout}, stderr: {stderr}"
    );
}

/// `gw upgrade` re-applies install for every already-installed (agent,scope)
/// pair, migrating to the current scheme. Targets that are not installed
/// stay untouched.
#[test]
fn upgrade_migrates_installed_targets_only() {
    use tempfile::tempdir;
    let dir = tempdir().unwrap();

    // Simulate an older Claude Code install: hook present + legacy marker
    // block in CLAUDE.md (the pre-rules-dir scheme). Codex NOT installed.
    let settings = dir.path().join(".claude/settings.local.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    std::fs::write(
        &settings,
        r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"gw hook claude-code"}]}]}}"#,
    )
    .unwrap();
    let claude_md = dir.path().join("CLAUDE.md");
    std::fs::write(
        &claude_md,
        "# Project rules\n\n<!-- gw:begin -->\n## old body\n<!-- gw:end -->\n",
    )
    .unwrap();

    let out = Command::new(bin())
        .arg("upgrade")
        .current_dir(dir.path())
        .env_remove("HOME")
        .output()
        .expect("spawn gw upgrade");
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Claude Code (local) was installed → migrated:
    // - Legacy CLAUDE.md block stripped, user content preserved.
    let after = std::fs::read_to_string(&claude_md).unwrap();
    assert!(after.contains("Project rules"));
    assert!(!after.contains("<!-- gw:begin -->"));
    // - New rules file in place.
    assert!(dir.path().join(".claude/rules/gw.md").exists());

    // Codex (local) was NOT installed → AGENTS.md must NOT have appeared.
    assert!(
        !dir.path().join("AGENTS.md").exists(),
        "upgrade must not install agents that were not previously installed"
    );
}

/// Local-only agent with --global (default) scope warns and skips.
#[test]
fn init_cline_global_warns_and_skips() {
    use tempfile::tempdir;
    let dir = tempdir().unwrap();
    let out = Command::new(bin())
        .args(["init", "--cline"])
        .current_dir(dir.path())
        .env("HOME", dir.path().to_str().unwrap())
        .output()
        .expect("spawn gw init");
    // Should succeed (skip is not an error) but warn on stderr.
    assert_eq!(out.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("does not support") && stderr.contains("global"),
        "expected scope-skip warning on stderr, got: {stderr}"
    );
}

/// Reproduces wrapper-command bugs (mainframer, ssh, custom shells):
/// child output occasionally embeds bare `\r` mid-line (terminal cursor
/// reset) which BufReader::lines does NOT split. Before the fix:
///   1. `BUILD FAILED` line preceded by `\r`-prefixed garbage failed to
///      classify (`^BUILD FAILED` regex) → summary said "no status reported".
///   2. Forwarded line still contained the `\r`, so terminals rendered
///      cascading offsets when ONLCR was disabled by the wrapper.
#[test]
fn forwards_cr_split_segments_as_separate_lines() {
    use std::io::Write;
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    // Three error lines, two of them joined by mid-line `\r` (as ssh / a
    // progress-bar layer might emit when overwriting).
    tmp.write_all(
        b"e: /a/Foo.kt:10:5 first error\rstale progress\n\
          e: /a/Foo.kt:20:5 second error\re: /a/Foo.kt:30:5 third error\n",
    )
    .unwrap();
    let path = tmp.path().to_path_buf();
    let (_code, stdout, _stderr) =
        run_gw(&["--no-log", "--no-heartbeat", "cat", path.to_str().unwrap()]);
    assert!(
        stdout.contains("first error"),
        "first error missing: {stdout}"
    );
    assert!(
        stdout.contains("second error"),
        "second error missing: {stdout}"
    );
    assert!(
        stdout.contains("third error"),
        "third error not split out from CR-joined segment: {stdout}"
    );
    // No bare `\r` in forwarded payload — would corrupt terminal layout.
    assert!(
        !stdout.contains('\r'),
        "stdout still contains bare `\\r`: {:?}",
        stdout
    );
}

#[test]
fn classifies_build_failed_when_preceded_by_cr_chunk() {
    use std::io::Write;
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    // Wrapper emits a transient progress segment then BUILD FAILED on the
    // same physical line (\r between them). Before the fix the whole line
    // was treated as `progress message\rBUILD FAILED in 11s` and the
    // anchored `^BUILD FAILED` regex failed.
    tmp.write_all(
        b"transient progress message\rBUILD FAILED in 11s\n\
          5427 actionable tasks: 1 executed, 5426 up-to-date\n",
    )
    .unwrap();
    let path = tmp.path().to_path_buf();
    let (_code, stdout, stderr) =
        run_gw(&["--no-log", "--no-heartbeat", "cat", path.to_str().unwrap()]);
    assert!(
        stdout.contains("BUILD FAILED in 11s"),
        "BUILD FAILED line not forwarded: {stdout}"
    );
    assert!(
        stderr.contains("BUILD FAILED") && !stderr.contains("no status reported"),
        "summary should say BUILD FAILED, got: {stderr}"
    );
    assert!(
        stderr.contains("5426 up-to-date"),
        "task stats missing from summary: {stderr}"
    );
}

#[test]
fn forwards_lines_to_pipe_with_plain_lf() {
    // When stdout is NOT a tty, gw should keep plain LF — emitting CRLF
    // would litter pipes / files committed to git with stray `\r`.
    let path = fixture("sample-failure.txt");
    let (_, stdout, _) = run_gw(&["--no-log", "--no-heartbeat", "cat", path.to_str().unwrap()]);
    assert!(
        !stdout.contains('\r'),
        "pipe output must use LF only, found CR: {:?}",
        stdout
    );
}

/// Smoke test with the full mainframer-shaped fixture: errors, FAILURE
/// block, BUILD FAILED, task summary, plus wrapper noise (sync, connection
/// notices). Verifies the summary is correct end-to-end.
#[test]
fn mainframer_wrapped_failure_summary_is_correct() {
    let path = fixture("sample-mainframer.txt");
    let (_, stdout, stderr) =
        run_gw(&["--no-log", "--no-heartbeat", "cat", path.to_str().unwrap()]);
    assert!(stdout.contains("FAILURE: Build failed"), "missing FAILURE");
    assert!(
        stdout.contains("BUILD FAILED in 11s"),
        "missing BUILD FAILED"
    );
    assert!(
        stderr.contains("BUILD FAILED") && !stderr.contains("no status reported"),
        "summary wrong: {stderr}"
    );
    assert!(stderr.contains("3 errors"), "error count wrong: {stderr}");
    assert!(
        stderr.contains("5426 up-to-date"),
        "task stats missing: {stderr}"
    );
}

/// Integration check: spawning gw with a fake gradlew script must result
/// in `--console=plain` reaching the child argv. We run a tiny shell
/// wrapper named `gradlew` that echoes its received args, then assert the
/// flag appears.
#[cfg(unix)]
#[test]
fn injects_console_plain_into_gradle_invocation() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("gradlew");
    {
        let mut f = std::fs::File::create(&script).unwrap();
        writeln!(f, "#!/bin/sh").unwrap();
        writeln!(f, "echo ARGS: \"$@\"").unwrap();
        writeln!(f, "echo BUILD SUCCESSFUL in 1s").unwrap();
    }
    let mut perm = std::fs::metadata(&script).unwrap().permissions();
    use std::os::unix::fs::PermissionsExt;
    perm.set_mode(0o755);
    std::fs::set_permissions(&script, perm).unwrap();

    let (_, stdout, _) = run_gw(&["--full", script.to_str().unwrap(), "assemble"]);
    assert!(
        stdout.contains("ARGS: --console=plain assemble"),
        "expected --console=plain injected, got: {stdout}"
    );
}

/// `--no-console-plain` opts out of injection.
#[cfg(unix)]
#[test]
fn no_console_plain_disables_injection() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("gradlew");
    {
        let mut f = std::fs::File::create(&script).unwrap();
        writeln!(f, "#!/bin/sh").unwrap();
        writeln!(f, "echo ARGS: \"$@\"").unwrap();
        writeln!(f, "echo BUILD SUCCESSFUL in 1s").unwrap();
    }
    use std::os::unix::fs::PermissionsExt;
    let mut perm = std::fs::metadata(&script).unwrap().permissions();
    perm.set_mode(0o755);
    std::fs::set_permissions(&script, perm).unwrap();

    let (_, stdout, _) = run_gw(&[
        "--full",
        "--no-console-plain",
        script.to_str().unwrap(),
        "assemble",
    ]);
    assert!(
        stdout.contains("ARGS: assemble"),
        "expected raw argv, got: {stdout}"
    );
    assert!(
        !stdout.contains("--console=plain"),
        "console flag should NOT be injected: {stdout}"
    );
}
