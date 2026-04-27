use once_cell::sync::Lazy;
use regex::Regex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineKind {
    KotlinError,
    KotlinWarning,
    JavaError,
    JavaWarning,
    GradleFailureStart,
    BuildSuccess,
    BuildFailed,
    BuildStats,
    TaskStart { name: String },
    TaskTerminal { name: String, status: String },
    Configure,
    TestPassed,
    TestFailed,
    TestSkipped,
    LintError,
    LintWarning,
    DaemonNoise,
    DownloadNoise,
    DeprecationNotice,
    Indented,
    Blank,
    Other,
}

static RE_KOTLIN_ERROR: Lazy<Regex> = Lazy::new(|| Regex::new(r"^e:\s").unwrap());
static RE_KOTLIN_WARNING: Lazy<Regex> = Lazy::new(|| Regex::new(r"^w:\s").unwrap());
static RE_JAVA_DIAGNOSTIC: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^.+\.java:\d+:\s+(error|warning):").unwrap());
static RE_TASK: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^> Task (:\S+?)(\s+(UP-TO-DATE|FAILED|SKIPPED|FROM-CACHE|NO-SOURCE))?\s*$")
        .unwrap()
});
static RE_CONFIGURE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^> Configure project ").unwrap());
static RE_BUILD_SUCCESS: Lazy<Regex> = Lazy::new(|| Regex::new(r"^BUILD SUCCESSFUL").unwrap());
static RE_BUILD_FAILED: Lazy<Regex> = Lazy::new(|| Regex::new(r"^BUILD FAILED").unwrap());
static RE_BUILD_STATS: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\d+ actionable tasks?:").unwrap());
static RE_FAILURE_START: Lazy<Regex> = Lazy::new(|| Regex::new(r"^FAILURE:\s").unwrap());
// Gradle test output shape: `ClassName > methodName PASSED`
// or `ClassName > nestedClass > methodName PASSED`.
// ClassName must look like a Java/Kotlin identifier: letters, digits, `_`, `$`,
// and `.` for fully-qualified names (e.g. `com.example.FooTest`).
// This rejects arbitrary prose like `Result count > 5 PASSED`.
static RE_TEST_PASSED: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[\w$][\w$.]*(\s>\s[^>]+){1,2}\s+PASSED\s*$").unwrap());
static RE_TEST_FAILED: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[\w$][\w$.]*(\s>\s[^>]+){1,2}\s+FAILED\s*$").unwrap());
static RE_TEST_SKIPPED: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[\w$][\w$.]*(\s>\s[^>]+){1,2}\s+SKIPPED\s*$").unwrap());
// Lint diagnostics: `path/to/File.kt:12:3: Error: message [RuleId]`
// Anchored at start; requires a non-space path and a trailing `[RuleId]`.
static RE_LINT_ERROR: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\S+:\d+(:\d+)?:\s+Error:\s.+\s\[\w[\w.]*\]\s*$").unwrap());
static RE_LINT_WARNING: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\S+:\d+(:\d+)?:\s+Warning:\s.+\s\[\w[\w.]*\]\s*$").unwrap());
static RE_DAEMON: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^(starting a gradle daemon|daemon will be stopped|gradle daemon)").unwrap()
});
static RE_DOWNLOAD: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)^(download |downloaded |downloading )").unwrap());
static RE_DEPRECATION: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(deprecated gradle features|use --warning-mode|deprecation warnings)").unwrap()
});
static RE_INDENTED: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s+\S").unwrap());

pub fn classify(line: &str) -> LineKind {
    if line.trim().is_empty() {
        return LineKind::Blank;
    }
    if RE_KOTLIN_ERROR.is_match(line) {
        return LineKind::KotlinError;
    }
    if RE_KOTLIN_WARNING.is_match(line) {
        return LineKind::KotlinWarning;
    }
    if let Some(caps) = RE_TASK.captures(line) {
        let name = caps
            .get(1)
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();
        let status = caps.get(3).map(|m| m.as_str().to_string());
        return match status {
            Some(s) => LineKind::TaskTerminal { name, status: s },
            None => LineKind::TaskStart { name },
        };
    }
    if RE_CONFIGURE.is_match(line) {
        return LineKind::Configure;
    }
    if RE_FAILURE_START.is_match(line) {
        return LineKind::GradleFailureStart;
    }
    if RE_BUILD_SUCCESS.is_match(line) {
        return LineKind::BuildSuccess;
    }
    if RE_BUILD_FAILED.is_match(line) {
        return LineKind::BuildFailed;
    }
    if RE_BUILD_STATS.is_match(line) {
        return LineKind::BuildStats;
    }
    if RE_TEST_FAILED.is_match(line) {
        return LineKind::TestFailed;
    }
    if RE_TEST_PASSED.is_match(line) {
        return LineKind::TestPassed;
    }
    if RE_TEST_SKIPPED.is_match(line) {
        return LineKind::TestSkipped;
    }
    if let Some(m) = RE_JAVA_DIAGNOSTIC.captures(line) {
        return match m.get(1).map(|x| x.as_str()) {
            Some("error") => LineKind::JavaError,
            Some("warning") => LineKind::JavaWarning,
            _ => LineKind::Other,
        };
    }
    if RE_LINT_ERROR.is_match(line) {
        return LineKind::LintError;
    }
    if RE_LINT_WARNING.is_match(line) {
        return LineKind::LintWarning;
    }
    if RE_DAEMON.is_match(line) {
        return LineKind::DaemonNoise;
    }
    if RE_DOWNLOAD.is_match(line) {
        return LineKind::DownloadNoise;
    }
    if RE_DEPRECATION.is_match(line) {
        return LineKind::DeprecationNotice;
    }
    if RE_INDENTED.is_match(line) {
        return LineKind::Indented;
    }
    LineKind::Other
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(line: &str) -> LineKind {
        classify(line)
    }

    #[test]
    fn kotlin_diagnostics() {
        assert_eq!(
            k("e: /a/b/Foo.kt:10:5 Unresolved reference: bar"),
            LineKind::KotlinError
        );
        assert_eq!(
            k("w: file:///x.kt:1:1 Parameter unused"),
            LineKind::KotlinWarning
        );
    }

    #[test]
    fn java_diagnostics() {
        assert_eq!(
            k("/a/Foo.java:10: error: cannot find symbol"),
            LineKind::JavaError
        );
        assert_eq!(
            k("/a/Foo.java:10: warning: [unchecked] cast"),
            LineKind::JavaWarning
        );
    }

    #[test]
    fn task_lines() {
        assert!(matches!(
            k("> Task :app:compileKotlin"),
            LineKind::TaskStart { .. }
        ));
        assert!(matches!(
            k("> Task :app:compileKotlin UP-TO-DATE"),
            LineKind::TaskTerminal { .. }
        ));
        assert!(matches!(
            k("> Task :app:compileKotlin FAILED"),
            LineKind::TaskTerminal { .. }
        ));
        if let LineKind::TaskStart { name } = k("> Task :app:compileKotlin") {
            assert_eq!(name, ":app:compileKotlin");
        } else {
            panic!()
        }
    }

    #[test]
    fn build_status() {
        assert_eq!(k("BUILD SUCCESSFUL in 2m 13s"), LineKind::BuildSuccess);
        assert_eq!(k("BUILD FAILED in 5s"), LineKind::BuildFailed);
        assert_eq!(
            k("47 actionable tasks: 12 executed, 33 up-to-date"),
            LineKind::BuildStats
        );
    }

    #[test]
    fn failure_block() {
        assert_eq!(
            k("FAILURE: Build failed with an exception."),
            LineKind::GradleFailureStart
        );
    }

    #[test]
    fn tests() {
        assert_eq!(k("FooTest > test bar PASSED"), LineKind::TestPassed);
        assert_eq!(k("FooTest > test bar FAILED"), LineKind::TestFailed);
        assert_eq!(k("FooTest > test bar SKIPPED"), LineKind::TestSkipped);
    }

    #[test]
    fn test_regex_anchoring() {
        // Valid Gradle test-output shapes — must match.
        assert_eq!(k("FooTest > bar PASSED"), LineKind::TestPassed);
        assert_eq!(
            k("com.example.FooTest > test bar with spaces PASSED"),
            LineKind::TestPassed
        );
        assert_eq!(
            k("com.example.FooTest$Inner > t PASSED"),
            LineKind::TestPassed
        );

        // Nested: ClassName > nestedClass > methodName PASSED
        assert_eq!(
            k("com.example.FooTest > Inner > doWork PASSED"),
            LineKind::TestPassed
        );

        // NOT a class name — must be rejected.
        assert_eq!(k("Result count > 5 PASSED"), LineKind::Other);
        // Line starting with digit is not a valid class identifier.
        assert_eq!(k("5 items > foo PASSED"), LineKind::Other);
    }

    #[test]
    fn lint_regex_anchoring() {
        // Valid lint error lines.
        assert_eq!(
            k("src/main/kotlin/Foo.kt:12:3: Error: message [RuleId]"),
            LineKind::LintError
        );
        assert_eq!(
            k("src/main/kotlin/Foo.kt:12: Warning: something [RuleId]"),
            LineKind::LintWarning
        );

        // False-positive: prefix "Note:" — must not match.
        assert_eq!(k("Note: line 5: Error: log message"), LineKind::Other);
        // No RuleId bracket — must not match.
        assert_eq!(k("src/Foo.kt:10: Error: bad code"), LineKind::Other);
        // Contains spaces in path — no path portion before first colon is non-whitespace.
        assert_eq!(
            k("path with spaces/Foo.kt:10: Error: bad [Rule]"),
            LineKind::Other
        );
    }

    #[test]
    fn noise() {
        assert_eq!(
            k("Starting a Gradle Daemon (subsequent builds will be faster)"),
            LineKind::DaemonNoise
        );
        assert_eq!(
            k("Download https://repo.maven.apache.org/foo"),
            LineKind::DownloadNoise
        );
        assert_eq!(
            k("Deprecated Gradle features were used in this build"),
            LineKind::DeprecationNotice
        );
    }

    #[test]
    fn indented_and_blank() {
        assert_eq!(k(""), LineKind::Blank);
        assert_eq!(k("    at FooTest.kt:15"), LineKind::Indented);
    }
}
