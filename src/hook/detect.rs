use once_cell::sync::Lazy;
use regex::Regex;

static ENV_PREFIX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r#"^(?:sudo\s+|env\s+|nice\s+|time\s+|[A-Z_][A-Z0-9_]*=(?:"[^"]*"|'[^']*'|\S+)\s+)+"#,
    )
    .unwrap()
});
static GRADLEW: Lazy<Regex> = Lazy::new(|| Regex::new(r"\bgradlew\b").unwrap());
static ALREADY_WRAPPED: Lazy<Regex> = Lazy::new(|| Regex::new(r"^gw\s").unwrap());

/// Commands whose quoted arguments should be treated as runnable shell code,
/// so we do NOT strip quoted segments when checking for `gradlew` inside them.
const SHELL_LIKE: &[&str] = &["ssh", "bash", "sh", "zsh", "fish"];

/// Extract the bare command name (first token after any env-var assignments).
fn command_name(cmd: &str) -> Option<String> {
    let stripped = ENV_PREFIX.replace(cmd, "").into_owned();
    let first = stripped.split_whitespace().next()?.to_string();
    // Strip leading path components so `./gradlew` → `gradlew`.
    let bare = first
        .trim_start_matches("./")
        .trim_start_matches('/')
        .to_string();
    Some(bare)
}

/// Remove all single- and double-quoted segments from `s`.
/// Used to strip quoted string literals so the `gradlew` word-boundary check
/// does not trigger on e.g. `echo "gradlew"`.
fn strip_quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                // Skip until closing double-quote (no escape handling needed for
                // our purpose — we only care about word presence).
                for inner in chars.by_ref() {
                    if inner == '"' {
                        break;
                    }
                }
            }
            '\'' => {
                for inner in chars.by_ref() {
                    if inner == '\'' {
                        break;
                    }
                }
            }
            other => out.push(other),
        }
    }
    out
}

pub fn detect_rewrite(command: &str) -> Option<String> {
    let trimmed = command.trim_start();
    if trimmed.is_empty() {
        return None;
    }
    let stripped = ENV_PREFIX.replace(trimmed, "");
    if ALREADY_WRAPPED.is_match(&stripped) {
        return None;
    }

    // Decide whether to strip quoted segments before the `gradlew` check.
    // For shell-like commands (ssh, bash, sh, …) the quoted block IS runnable
    // code that may contain `./gradlew`, so we keep it intact.
    let haystack: String;
    let search_in: &str = if command_name(&stripped)
        .map(|n| SHELL_LIKE.contains(&n.as_str()))
        .unwrap_or(false)
    {
        &stripped
    } else {
        haystack = strip_quoted(&stripped);
        &haystack
    };

    if !GRADLEW.is_match(search_in) {
        return None;
    }
    Some(format!("gw {}", trimmed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_simple_gradlew() {
        assert_eq!(
            detect_rewrite("./gradlew assemble").as_deref(),
            Some("gw ./gradlew assemble")
        );
    }

    #[test]
    fn rewrites_mainframer_wrapper() {
        assert_eq!(
            detect_rewrite("./mainframer ./gradlew assembleRelease").as_deref(),
            Some("gw ./mainframer ./gradlew assembleRelease")
        );
    }

    #[test]
    fn rewrites_ssh_remote_invocation() {
        assert_eq!(
            detect_rewrite("ssh build-host './gradlew test'").as_deref(),
            Some("gw ssh build-host './gradlew test'")
        );
    }

    #[test]
    fn skips_already_wrapped() {
        assert!(detect_rewrite("gw ./gradlew assemble").is_none());
    }

    #[test]
    fn strips_env_prefix() {
        assert_eq!(
            detect_rewrite("FOO=bar ./gradlew test").as_deref(),
            Some("gw FOO=bar ./gradlew test")
        );
        assert_eq!(
            detect_rewrite("sudo ./gradlew test").as_deref(),
            Some("gw sudo ./gradlew test")
        );
        assert_eq!(
            detect_rewrite("time nice ./gradlew test").as_deref(),
            Some("gw time nice ./gradlew test")
        );
    }

    #[test]
    fn ignores_non_gradle_commands() {
        assert!(detect_rewrite("git status").is_none());
        assert!(detect_rewrite("npm run build").is_none());
        assert!(detect_rewrite("./build.sh").is_none());
    }

    #[test]
    fn ignores_substring_match() {
        assert!(detect_rewrite("echo not-gradlewish").is_none());
        assert!(detect_rewrite("./mygradlewrapper foo").is_none());
    }

    // C5 additions ────────────────────────────────────────────────────────────

    #[test]
    fn ignores_gradlew_in_double_quotes() {
        // `echo "gradlew"` must NOT be rewritten — the word is only in a literal string.
        assert!(detect_rewrite(r#"echo "gradlew""#).is_none());
    }

    #[test]
    fn ignores_gradlew_in_single_quotes() {
        assert!(detect_rewrite("echo 'gradlew'").is_none());
    }

    #[test]
    fn rewrites_bash_with_quoted_gradlew() {
        // bash/sh/zsh treat the quoted argument as code — must still wrap.
        let result = detect_rewrite("bash -c './gradlew test'");
        assert!(
            result.is_some(),
            "bash -c './gradlew test' should be wrapped"
        );
    }

    #[test]
    fn ignores_cat_with_quoted_gradlew_bat() {
        // `cat 'gradlew.bat'` — gradlew is inside a quote for a non-shell command.
        assert!(detect_rewrite("cat 'gradlew.bat'").is_none());
    }
}
