use once_cell::sync::Lazy;
use regex::Regex;

static ENV_PREFIX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r#"^(?:sudo\s+|env\s+|nice\s+|time\s+|[A-Z_][A-Z0-9_]*=(?:"[^"]*"|'[^']*'|\S+)\s+)+"#,
    )
    .unwrap()
});
static ALREADY_WRAPPED: Lazy<Regex> = Lazy::new(|| Regex::new(r"^gw\s").unwrap());

/// Strip surrounding `'`/`"` and leading path components, return the bare
/// executable name (e.g. `'./gradlew` → `gradlew`, `/usr/bin/foo` → `foo`).
fn bare_token(token: &str) -> &str {
    let t = token.trim_matches(|c| c == '"' || c == '\'');
    let t = t.trim_start_matches("./");
    match t.rfind('/') {
        Some(i) => &t[i + 1..],
        None => t,
    }
}

/// Drop heredoc bodies from a segment, keeping the command line itself.
///
/// Everything a heredoc feeds to a command is data (a python script, a config,
/// a commit message), so `gradlew` mentioned there must not make the segment
/// look like a build invocation.
fn strip_heredoc_bodies(seg: &str) -> String {
    let mut out = String::new();
    let mut lines = seg.split('\n');
    while let Some(line) = lines.next() {
        out.push_str(line);
        out.push('\n');
        // Collect delimiters opened on this line, then skip their bodies.
        let mut rest = line;
        let mut delims = Vec::new();
        while let Some(i) = rest.find("<<") {
            if let Some(d) = heredoc_delimiter(&rest[i..]) {
                delims.push(d);
            }
            rest = &rest[i + 2..];
        }
        for delim in delims {
            for body in lines.by_ref() {
                if body.trim() == delim {
                    break;
                }
            }
        }
    }
    out
}

/// True iff `seg` invokes `gradlew` as a runnable command — either directly
/// (`./gradlew assemble`) or via a wrapper binary that takes a *path* to
/// gradlew as its argument (`mainframer ./gradlew test`,
/// `bash -c './gradlew test'`, `ssh host './gradlew test'`).
///
/// Plain bareword matches like `grep gradlew`, `pgrep gradlew`,
/// `find . -name gradlew`, or `echo "gradlew"` do NOT count: gradlew there
/// is data, not an executable. The path requirement (`./`, `/`, `../`
/// prefix) is what filters those out.
fn segment_invokes_gradlew(seg: &str) -> bool {
    let seg = strip_heredoc_bodies(seg);
    let stripped = ENV_PREFIX.replace(seg.trim_start(), "").into_owned();
    let mut tokens = stripped.split_whitespace();
    let Some(first) = tokens.next() else {
        return false;
    };
    if bare_token(first) == "gradlew" {
        return true;
    }
    for t in tokens {
        let cleaned = t.trim_matches(|c| c == '"' || c == '\'');
        let looks_like_path =
            cleaned.starts_with("./") || cleaned.starts_with('/') || cleaned.starts_with("../");
        if looks_like_path && bare_token(cleaned) == "gradlew" {
            return true;
        }
    }
    false
}

/// Parse a heredoc redirection at the start of `rest` (`<<EOF`, `<<-'EOF'`,
/// `<<< "x"` is *not* a heredoc). Returns the delimiter word if found.
///
/// Only the operator and its delimiter are inspected; quoting of the
/// delimiter (`<<'EOF'`, `<<"EOF"`) does not change where the body ends.
fn heredoc_delimiter(rest: &str) -> Option<String> {
    let after_op = rest.strip_prefix("<<")?;
    // `<<<` is a here-string, not a heredoc: no body to skip.
    if after_op.starts_with('<') {
        return None;
    }
    let after_dash = after_op.strip_prefix('-').unwrap_or(after_op);
    let word = after_dash.trim_start();
    let delim: String = if let Some(q) = word.strip_prefix('\'') {
        q.chars().take_while(|&c| c != '\'').collect()
    } else if let Some(q) = word.strip_prefix('"') {
        q.chars().take_while(|&c| c != '"').collect()
    } else {
        word.chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '.')
            .collect()
    };
    if delim.is_empty() {
        None
    } else {
        Some(delim)
    }
}

/// Split a shell command line into (segment, separator-after) pairs at
/// top-level `;`, `&&`, `||`, `|`, `&`. Respects single/double quotes,
/// backslash escapes and heredoc bodies so separators inside them are
/// preserved.
fn split_segments(cmd: &str) -> Vec<(String, String)> {
    let mut segs = Vec::new();
    let mut cur = String::new();
    let mut chars = cmd.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;
    // Delimiters of heredocs opened on the current line, in order.
    let mut pending_heredocs: Vec<String> = Vec::new();
    while let Some(c) = chars.next() {
        // A heredoc body starts after the newline that ends the opening line and
        // runs verbatim until its delimiter — shell separators inside it are
        // plain text (e.g. a python script containing `;` or `./gradlew`).
        if c == '\n' && !pending_heredocs.is_empty() && !in_single && !in_double {
            cur.push(c);
            let mut trailing_newline = false;
            for delim in std::mem::take(&mut pending_heredocs) {
                let mut line = String::new();
                trailing_newline = false;
                loop {
                    match chars.next() {
                        Some('\n') => {
                            let done = line.trim() == delim;
                            cur.push_str(&line);
                            line.clear();
                            if done {
                                trailing_newline = true;
                                break;
                            }
                            cur.push('\n');
                        }
                        Some(ch) => line.push(ch),
                        None => {
                            cur.push_str(&line);
                            break;
                        }
                    }
                }
            }
            // The command ends with its heredoc: close the segment so whatever
            // follows on the next line is analysed on its own.
            if trailing_newline {
                segs.push((std::mem::take(&mut cur), "\n".to_string()));
            }
            continue;
        }
        if in_single {
            cur.push(c);
            if c == '\'' {
                in_single = false;
            }
            continue;
        }
        if in_double {
            cur.push(c);
            if c == '\\' {
                if let Some(&n) = chars.peek() {
                    cur.push(n);
                    chars.next();
                }
            } else if c == '"' {
                in_double = false;
            }
            continue;
        }
        match c {
            '<' if chars.peek() == Some(&'<') => {
                // Remember the delimiter; the body is consumed at the newline.
                let rest: String = std::iter::once('<').chain(chars.clone()).collect();
                if let Some(delim) = heredoc_delimiter(&rest) {
                    pending_heredocs.push(delim);
                }
                cur.push(c);
            }
            '\'' => {
                in_single = true;
                cur.push(c);
            }
            '"' => {
                in_double = true;
                cur.push(c);
            }
            '\\' => {
                cur.push(c);
                if let Some(&n) = chars.peek() {
                    cur.push(n);
                    chars.next();
                }
            }
            ';' => {
                segs.push((std::mem::take(&mut cur), ";".to_string()));
            }
            // Newline separates commands just like `;` — without this a
            // multi-line script would be treated as one segment and `gw` would
            // land on its first line instead of the gradlew invocation.
            '\n' => {
                segs.push((std::mem::take(&mut cur), "\n".to_string()));
            }
            '&' => {
                if chars.peek() == Some(&'&') {
                    chars.next();
                    segs.push((std::mem::take(&mut cur), "&&".to_string()));
                } else if cur.ends_with('>') || chars.peek() == Some(&'>') {
                    // Redirect form: `2>&1`, `>&2`, `&>file`. Keep `&` literal.
                    cur.push(c);
                } else {
                    segs.push((std::mem::take(&mut cur), "&".to_string()));
                }
            }
            '|' => {
                if chars.peek() == Some(&'|') {
                    chars.next();
                    segs.push((std::mem::take(&mut cur), "||".to_string()));
                } else {
                    segs.push((std::mem::take(&mut cur), "|".to_string()));
                }
            }
            _ => cur.push(c),
        }
    }
    segs.push((cur, String::new()));
    segs
}

/// Rewrite a single shell segment (no top-level separators inside).
/// Returns `Some(wrapped)` if the segment names gradlew and isn't already wrapped.
fn rewrite_segment(seg: &str) -> Option<String> {
    let trimmed = seg.trim_start();
    if trimmed.is_empty() {
        return None;
    }
    let leading = &seg[..seg.len() - trimmed.len()];
    let stripped = ENV_PREFIX.replace(trimmed, "").into_owned();
    if ALREADY_WRAPPED.is_match(&stripped) {
        return None;
    }
    if !segment_invokes_gradlew(&stripped) {
        return None;
    }
    Some(format!("{}gw {}", leading, trimmed))
}

/// Detect the pattern `./gradlew ... | tail|head ...` (and `&&`/`||`/`;` are
/// fine — only a direct `|` from a gradlew segment counts as truncation of
/// build output). Returns a fixed warning string if found, else `None`.
///
/// Only `tail` and `head` are flagged. Other filters (`grep`, `sed`, `awk`)
/// have legitimate uses on gradle output (e.g. `| grep ERROR`) and would
/// produce too many false positives.
pub fn detect_truncation(command: &str) -> Option<&'static str> {
    let segments = split_segments(command);
    for i in 0..segments.len().saturating_sub(1) {
        let (seg, sep) = &segments[i];
        if sep != "|" {
            continue;
        }
        if !segment_invokes_gradlew(seg.trim_start()) {
            continue;
        }
        let next = segments[i + 1].0.trim_start();
        let first = next.split_whitespace().next().unwrap_or("");
        let bare = bare_token(first);
        if bare == "tail" || bare == "head" {
            return Some(
                "gw output is already filtered — piping through tail/head drops the leading error/stacktrace. Re-run without the truncator and read the full output."
            );
        }
    }
    None
}

pub fn detect_rewrite(command: &str) -> Option<String> {
    if command.trim().is_empty() {
        return None;
    }
    let segments = split_segments(command);
    let mut any = false;
    let mut out = String::new();
    for (seg, sep) in &segments {
        match rewrite_segment(seg) {
            Some(new) => {
                out.push_str(&new);
                any = true;
            }
            None => out.push_str(seg),
        }
        out.push_str(sep);
    }
    if any {
        Some(out)
    } else {
        None
    }
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
    fn leaves_heredoc_body_alone() {
        // Script text inside a heredoc is data: mentioning gradlew there must
        // not inject `gw` into the middle of the script.
        let cmd = "cat > run.sh << 'EOF'\n./gradlew assemble\nEOF";
        assert_eq!(detect_rewrite(cmd), None);
    }

    #[test]
    fn leaves_heredoc_with_separators_alone() {
        // Separators inside the body must not split segments either — this used
        // to corrupt python/shell scripts passed via heredoc.
        let cmd = "python3 - << 'PY'\ns = open(p).read()\nprint('a && b | c; d')\nPY";
        assert_eq!(detect_rewrite(cmd), None);
    }

    #[test]
    fn rewrites_command_after_heredoc_body() {
        let cmd = "cat << 'EOF'\ntext\nEOF\n./gradlew test";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("cat << 'EOF'\ntext\nEOF\ngw ./gradlew test")
        );
    }

    #[test]
    fn rewrites_gradlew_before_heredoc() {
        let cmd = "./gradlew test << 'EOF'\ninput\nEOF";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("gw ./gradlew test << 'EOF'\ninput\nEOF")
        );
    }

    #[test]
    fn here_string_is_not_heredoc() {
        // `<<<` is a here-string: no body follows, so the next segment is code.
        let cmd = "grep x <<< \"data\" && ./gradlew build";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("grep x <<< \"data\" && gw ./gradlew build")
        );
    }

    #[test]
    fn tab_indented_heredoc_delimiter() {
        let cmd = "cat <<-EOF\n./gradlew assemble\n\tEOF";
        assert_eq!(detect_rewrite(cmd), None);
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
        assert!(detect_rewrite(r#"echo "gradlew""#).is_none());
    }

    #[test]
    fn ignores_gradlew_in_single_quotes() {
        assert!(detect_rewrite("echo 'gradlew'").is_none());
    }

    #[test]
    fn rewrites_bash_with_quoted_gradlew() {
        let result = detect_rewrite("bash -c './gradlew test'");
        assert!(
            result.is_some(),
            "bash -c './gradlew test' should be wrapped"
        );
    }

    #[test]
    fn ignores_cat_with_quoted_gradlew_bat() {
        assert!(detect_rewrite("cat 'gradlew.bat'").is_none());
    }

    // Compound shell separators ───────────────────────────────────────────────

    #[test]
    fn rewrites_each_segment_in_semicolon_chain() {
        assert_eq!(
            detect_rewrite("./gradlew --stop 2>/dev/null; ./gradlew :app:assembleArmDebug")
                .as_deref(),
            Some("gw ./gradlew --stop 2>/dev/null; gw ./gradlew :app:assembleArmDebug")
        );
    }

    #[test]
    fn rewrites_each_segment_in_and_chain() {
        assert_eq!(
            detect_rewrite("./gradlew clean && ./gradlew test").as_deref(),
            Some("gw ./gradlew clean && gw ./gradlew test")
        );
    }

    #[test]
    fn rewrites_each_segment_in_or_chain() {
        assert_eq!(
            detect_rewrite("./gradlew test || ./gradlew test --info").as_deref(),
            Some("gw ./gradlew test || gw ./gradlew test --info")
        );
    }

    #[test]
    fn rewrites_only_gradlew_segment_in_mixed_chain() {
        assert_eq!(
            detect_rewrite("git pull && ./gradlew build").as_deref(),
            Some("git pull && gw ./gradlew build")
        );
        assert_eq!(
            detect_rewrite("echo go; ./gradlew test").as_deref(),
            Some("echo go; gw ./gradlew test")
        );
    }

    #[test]
    fn skips_already_wrapped_per_segment() {
        assert_eq!(
            detect_rewrite("gw ./gradlew clean; ./gradlew test").as_deref(),
            Some("gw ./gradlew clean; gw ./gradlew test")
        );
    }

    #[test]
    fn returns_none_when_no_segment_has_gradlew() {
        assert!(detect_rewrite("git pull && npm test").is_none());
    }

    #[test]
    fn does_not_split_inside_single_quotes() {
        assert_eq!(
            detect_rewrite("ssh host './gradlew a; ./gradlew b'").as_deref(),
            Some("gw ssh host './gradlew a; ./gradlew b'")
        );
    }

    #[test]
    fn does_not_split_inside_double_quotes() {
        assert_eq!(
            detect_rewrite(r#"bash -c "./gradlew a && ./gradlew b""#).as_deref(),
            Some(r#"gw bash -c "./gradlew a && ./gradlew b""#)
        );
    }

    #[test]
    fn handles_pipe_separator() {
        assert_eq!(
            detect_rewrite("./gradlew test | tee log.txt").as_deref(),
            Some("gw ./gradlew test | tee log.txt")
        );
    }

    #[test]
    fn does_not_split_on_redirect_ampersand() {
        // `2>&1` is a redirect, not a background separator. Must not split.
        assert_eq!(
            detect_rewrite("./gradlew test 2>&1 | tee log.txt").as_deref(),
            Some("gw ./gradlew test 2>&1 | tee log.txt")
        );
    }

    #[test]
    fn handles_background_ampersand() {
        assert_eq!(
            detect_rewrite("./gradlew --stop & ./gradlew test").as_deref(),
            Some("gw ./gradlew --stop & gw ./gradlew test")
        );
    }

    // Bareword false-positive regressions ─────────────────────────────────────

    #[test]
    fn ignores_ssh_grep_for_gradlew_pattern() {
        // The pipes are inside the SSH-quoted arg, so they belong to the
        // remote shell. Locally this is a single segment whose command is
        // `ssh`, and `gradlew` only appears as a grep pattern (no path
        // prefix) — must NOT wrap.
        assert!(detect_rewrite(r#"ssh abuild "ps aux | grep gradlew | grep -v grep""#).is_none());
    }

    #[test]
    fn ignores_top_level_grep_for_gradlew_pattern() {
        assert!(detect_rewrite("ps aux | grep gradlew | grep -v grep").is_none());
    }

    #[test]
    fn ignores_pgrep_gradlew() {
        assert!(detect_rewrite("pgrep gradlew").is_none());
    }

    #[test]
    fn ignores_find_name_gradlew() {
        assert!(detect_rewrite("find . -name gradlew").is_none());
    }

    #[test]
    fn rewrites_absolute_path_to_gradlew() {
        assert_eq!(
            detect_rewrite("/repo/proj/gradlew assemble").as_deref(),
            Some("gw /repo/proj/gradlew assemble")
        );
    }

    #[test]
    fn rewrites_parent_relative_path_to_gradlew() {
        assert_eq!(
            detect_rewrite("../proj/gradlew assemble").as_deref(),
            Some("gw ../proj/gradlew assemble")
        );
    }

    // Truncation detection ────────────────────────────────────────────────────

    #[test]
    fn flags_gradlew_piped_to_tail() {
        assert!(detect_truncation("./gradlew test | tail -n 80").is_some());
    }

    #[test]
    fn flags_gradlew_piped_to_head() {
        assert!(detect_truncation("./gradlew test | head -n 50").is_some());
    }

    #[test]
    fn flags_gradlew_with_redirect_then_tail() {
        assert!(detect_truncation("./gradlew :app:assemble 2>&1 | tail -n 200").is_some());
    }

    #[test]
    fn flags_tail_in_middle_of_chain() {
        // `./gradlew clean && ./gradlew test | tail -n 50` — second gradlew piped to tail.
        assert!(detect_truncation("./gradlew clean && ./gradlew test | tail -n 50").is_some());
    }

    #[test]
    fn does_not_flag_grep_filter() {
        assert!(detect_truncation("./gradlew test | grep ERROR").is_none());
    }

    #[test]
    fn does_not_flag_tail_on_non_gradlew() {
        assert!(detect_truncation("git log | tail -n 20").is_none());
    }

    #[test]
    fn does_not_flag_plain_gradlew() {
        assert!(detect_truncation("./gradlew test").is_none());
    }

    #[test]
    fn does_not_flag_tail_after_semicolon() {
        // Separate command, not piping gradlew output.
        assert!(detect_truncation("./gradlew test; tail -n 10 build.log").is_none());
    }

    #[test]
    fn does_not_flag_tail_inside_quoted_arg() {
        // The `| tail` is inside the SSH-quoted arg — locally it's a single
        // segment whose command is `ssh`, not gradlew.
        assert!(detect_truncation(r#"ssh host "./gradlew test | tail -n 20""#).is_none());
    }
}
