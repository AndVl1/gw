use crate::parser::{classify, LineKind};

#[derive(Debug, Default)]
pub struct Stats {
    pub errors: u32,
    pub warnings: u32,
    pub deprecations: u32,
    pub tests_passed: u32,
    pub tests_failed: u32,
    pub tests_skipped: u32,
    pub tasks_executed: u32,
    pub tasks_up_to_date: u32,
    pub tasks_from_cache: u32,
    pub tasks_skipped: u32,
    pub tasks_no_source: u32,
    pub tasks_failed: u32,
    pub build_success: bool,
    pub build_failed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Forward,
    Suppress,
}

/// Maximum lines forwarded while in `InFailureBlock` before we give up and
/// return to `Normal`.  Prevents unbounded forwarding when `BUILD FAILED` is
/// never emitted (e.g. the process was killed mid-output).
const FAILURE_BLOCK_MAX_LINES: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Normal,
    /// Carries a remaining-line counter so we cap forwarding at
    /// `FAILURE_BLOCK_MAX_LINES` even when the terminal `BUILD FAILED` line is
    /// never emitted.
    InFailureBlock(usize),
    InErrorContinuation(usize),
}

pub struct Processor {
    state: State,
    pub stats: Stats,
    pub current_task: Option<String>,
    /// Cumulative count of task lifecycle events seen — every `> Task :foo`
    /// (start) and every terminal status (UP-TO-DATE / FROM-CACHE / SKIPPED /
    /// NO-SOURCE / FAILED).  Surfaced in heartbeat as a progress signal.
    pub progress_count: u32,
    pub mode: Mode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Default,
    Quiet,
    WithWarnings,
}

const ERROR_CONTINUATION_LINES: usize = 30;

impl Processor {
    pub fn new(mode: Mode) -> Self {
        Self {
            state: State::Normal,
            stats: Stats::default(),
            current_task: None,
            progress_count: 0,
            mode,
        }
    }

    pub fn process(&mut self, line: &str) -> Decision {
        let kind = classify(line);
        match kind {
            LineKind::KotlinError | LineKind::JavaError | LineKind::LintError => {
                self.stats.errors += 1;
                self.state = State::InErrorContinuation(ERROR_CONTINUATION_LINES);
                Decision::Forward
            }
            LineKind::KotlinWarning | LineKind::JavaWarning | LineKind::LintWarning => {
                self.stats.warnings += 1;
                self.state = State::Normal;
                if self.mode == Mode::WithWarnings {
                    Decision::Forward
                } else {
                    Decision::Suppress
                }
            }
            LineKind::GradleFailureStart => {
                self.state = State::InFailureBlock(FAILURE_BLOCK_MAX_LINES);
                Decision::Forward
            }
            LineKind::BuildSuccess => {
                self.stats.build_success = true;
                self.state = State::Normal;
                Decision::Forward
            }
            LineKind::BuildFailed => {
                self.stats.build_failed = true;
                self.state = State::Normal;
                Decision::Forward
            }
            LineKind::BuildStats => {
                self.parse_stats(line);
                Decision::Suppress
            }
            LineKind::TaskStart { name } => {
                self.current_task = Some(name);
                self.progress_count = self.progress_count.saturating_add(1);
                Decision::Suppress
            }
            LineKind::TaskTerminal { status, name } => {
                self.progress_count = self.progress_count.saturating_add(1);
                match status.as_str() {
                    "UP-TO-DATE" => self.stats.tasks_up_to_date += 1,
                    "FROM-CACHE" => self.stats.tasks_from_cache += 1,
                    "SKIPPED" => self.stats.tasks_skipped += 1,
                    "NO-SOURCE" => self.stats.tasks_no_source += 1,
                    "FAILED" => {
                        self.stats.tasks_failed += 1;
                        self.state = State::InErrorContinuation(ERROR_CONTINUATION_LINES);
                        return Decision::Forward;
                    }
                    _ => {}
                }
                if Some(&name) == self.current_task.as_ref() {
                    self.current_task = None;
                }
                Decision::Suppress
            }
            LineKind::Configure => Decision::Suppress,
            LineKind::TestPassed => {
                self.stats.tests_passed += 1;
                Decision::Suppress
            }
            LineKind::TestFailed => {
                self.stats.tests_failed += 1;
                self.state = State::InErrorContinuation(ERROR_CONTINUATION_LINES);
                Decision::Forward
            }
            LineKind::TestSkipped => {
                self.stats.tests_skipped += 1;
                Decision::Suppress
            }
            LineKind::DaemonNoise | LineKind::DownloadNoise => Decision::Suppress,
            LineKind::DeprecationNotice => {
                self.stats.deprecations += 1;
                Decision::Suppress
            }
            LineKind::Indented => match self.state {
                State::InFailureBlock(n) if n > 0 => {
                    self.state = State::InFailureBlock(n - 1);
                    Decision::Forward
                }
                State::InFailureBlock(_) => {
                    self.state = State::Normal;
                    Decision::Suppress
                }
                State::InErrorContinuation(n) if n > 0 => {
                    self.state = State::InErrorContinuation(n - 1);
                    Decision::Forward
                }
                _ => Decision::Suppress,
            },
            LineKind::Blank => match self.state {
                State::InFailureBlock(n) if n > 0 => {
                    self.state = State::InFailureBlock(n - 1);
                    Decision::Forward
                }
                State::InFailureBlock(_) => {
                    self.state = State::Normal;
                    Decision::Suppress
                }
                _ => {
                    self.state = State::Normal;
                    Decision::Suppress
                }
            },
            LineKind::Other => match self.state {
                State::InFailureBlock(n) if n > 0 => {
                    self.state = State::InFailureBlock(n - 1);
                    Decision::Forward
                }
                State::InFailureBlock(_) => {
                    self.state = State::Normal;
                    Decision::Suppress
                }
                State::InErrorContinuation(_) => {
                    self.state = State::Normal;
                    Decision::Suppress
                }
                _ => Decision::Suppress,
            },
        }
    }

    fn parse_stats(&mut self, line: &str) {
        let body = line.split_once(':').map(|(_, b)| b).unwrap_or(line);
        for part in body.split(',') {
            let part = part.trim();
            let (num, label) = match part.split_once(' ') {
                Some(p) => p,
                None => continue,
            };
            let n: u32 = match num.parse() {
                Ok(n) => n,
                Err(_) => continue,
            };
            match label {
                "executed" => self.stats.tasks_executed = n,
                "up-to-date" => self.stats.tasks_up_to_date = n,
                "from cache" => self.stats.tasks_from_cache = n,
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwards_kotlin_error_and_continuation() {
        let mut p = Processor::new(Mode::Default);
        assert_eq!(
            p.process("e: /a/Foo.kt:1:1 Unresolved reference"),
            Decision::Forward
        );
        assert_eq!(p.process("    at line"), Decision::Forward);
        assert_eq!(p.process("not indented"), Decision::Suppress);
    }

    #[test]
    fn suppresses_warnings_by_default() {
        let mut p = Processor::new(Mode::Default);
        assert_eq!(
            p.process("w: /a/Foo.kt:1:1 Unused parameter"),
            Decision::Suppress
        );
        assert_eq!(p.stats.warnings, 1);
    }

    #[test]
    fn forwards_warnings_when_requested() {
        let mut p = Processor::new(Mode::WithWarnings);
        assert_eq!(
            p.process("w: /a/Foo.kt:1:1 Unused parameter"),
            Decision::Forward
        );
    }

    #[test]
    fn task_tracks_current() {
        let mut p = Processor::new(Mode::Default);
        p.process("> Task :app:compileKotlin");
        assert_eq!(p.current_task.as_deref(), Some(":app:compileKotlin"));
        p.process("> Task :app:compileKotlin UP-TO-DATE");
        assert_eq!(p.current_task, None);
        assert_eq!(p.stats.tasks_up_to_date, 1);
    }

    #[test]
    fn progress_count_increments_on_task_lifecycle() {
        let mut p = Processor::new(Mode::Default);
        assert_eq!(p.progress_count, 0);
        p.process("> Task :app:compileKotlin");
        assert_eq!(p.progress_count, 1);
        p.process("> Task :app:test UP-TO-DATE");
        assert_eq!(p.progress_count, 2);
        p.process("> Task :app:assemble FROM-CACHE");
        assert_eq!(p.progress_count, 3);
        // Non-task line must not bump.
        p.process("Some other output");
        assert_eq!(p.progress_count, 3);
    }

    #[test]
    fn failure_block_is_forwarded_entirely() {
        let mut p = Processor::new(Mode::Default);
        assert_eq!(
            p.process("FAILURE: Build failed with an exception."),
            Decision::Forward
        );
        assert_eq!(p.process("* What went wrong:"), Decision::Forward);
        assert_eq!(p.process("    Could not resolve foo"), Decision::Forward);
        assert_eq!(p.process(""), Decision::Forward);
        assert_eq!(p.process("BUILD FAILED in 5s"), Decision::Forward);
        assert_eq!(p.process("after"), Decision::Suppress);
    }

    #[test]
    fn failure_block_caps_at_200_lines() {
        let mut p = Processor::new(Mode::Default);
        // Enter failure block.
        assert_eq!(
            p.process("FAILURE: Build failed with an exception."),
            Decision::Forward
        );

        // Feed 300 unrelated non-blank, non-indented lines after the start.
        // The processor should stop forwarding at or before line 200.
        let mut forwarded_after = 0usize;
        let mut first_suppressed = None;
        for i in 0..300usize {
            // Use non-blank, non-indented "Other" lines to keep triggering the
            // InFailureBlock counter path.
            let d = p.process("some unrelated output line");
            if matches!(d, Decision::Forward) {
                forwarded_after += 1;
            } else if first_suppressed.is_none() {
                first_suppressed = Some(i);
            }
        }
        // We must have returned to Normal well before line 300.
        assert!(
            forwarded_after <= 200,
            "forwarded {forwarded_after} lines — should be capped at 200"
        );
        assert!(
            first_suppressed.is_some(),
            "never suppressed — failure block never ended"
        );
        // State should be Normal after the cap.
        assert_eq!(p.process("BUILD FAILED in 5s"), Decision::Forward); // recognised as terminal
    }

    #[test]
    fn build_success_forwarded() {
        let mut p = Processor::new(Mode::Default);
        assert_eq!(p.process("BUILD SUCCESSFUL in 2m 13s"), Decision::Forward);
        assert!(p.stats.build_success);
    }

    #[test]
    fn parses_actionable_tasks_summary() {
        let mut p = Processor::new(Mode::Default);
        p.process("47 actionable tasks: 12 executed, 33 up-to-date, 2 from cache");
        assert_eq!(p.stats.tasks_executed, 12);
        assert_eq!(p.stats.tasks_up_to_date, 33);
        assert_eq!(p.stats.tasks_from_cache, 2);
    }

    #[test]
    fn daemon_and_downloads_suppressed() {
        let mut p = Processor::new(Mode::Default);
        assert_eq!(
            p.process("Starting a Gradle Daemon (subsequent builds will be faster)"),
            Decision::Suppress
        );
        assert_eq!(p.process("Download https://repo/foo"), Decision::Suppress);
    }

    #[test]
    fn test_failures_forwarded_with_stacktrace() {
        let mut p = Processor::new(Mode::Default);
        assert_eq!(p.process("FooTest > bar FAILED"), Decision::Forward);
        assert_eq!(p.process("    java.lang.AssertionError"), Decision::Forward);
        assert_eq!(p.stats.tests_failed, 1);
    }
}
