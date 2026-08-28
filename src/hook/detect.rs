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

/// One top-level shell command plus its trailing syntax.
///
/// Reconstruction invariant: concatenating `text + sep + tail` over all
/// segments reproduces the original command byte-for-byte.
struct Segment {
    /// The command line itself — never contains heredoc bodies.
    text: String,
    /// Separator that ended the segment (`;`, `&&`, `|`, `\n`, ... or empty).
    sep: String,
    /// Heredoc bodies physically following this segment's line, verbatim
    /// (body lines and delimiter lines, newlines included). Data, not code.
    tail: String,
}

/// A heredoc opened on the current line, awaiting its body.
struct Heredoc {
    delim: String,
    /// `<<-` form: leading tabs are stripped before delimiter comparison.
    strip_tabs: bool,
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

/// Lex a heredoc delimiter word from the main char stream, right after the
/// `<<` operator was consumed. Every consumed char is also pushed to `cur`
/// verbatim so the segment text stays byte-identical to the input.
///
/// The word follows POSIX quote-removal: adjacent parts concatenate
/// (`<<'E''OF'` → `EOF`, `<<\EOF` → `EOF`, `<<'END'foo` → `ENDfoo`) and any
/// character except an unquoted shell metacharacter may appear
/// (`END-OF-DATA` is one word). Quoting changes body expansion, not where
/// the body ends, so only the resulting text matters here.
fn lex_heredoc_word(
    chars: &mut std::iter::Peekable<std::str::Chars>,
    cur: &mut String,
) -> Option<Heredoc> {
    let strip_tabs = if chars.peek() == Some(&'-') {
        cur.push(chars.next().expect("peeked"));
        true
    } else {
        false
    };
    while matches!(chars.peek(), Some(' ') | Some('\t')) {
        cur.push(chars.next().expect("peeked"));
    }
    let mut delim = String::new();
    loop {
        match chars.peek() {
            Some('\'') => {
                cur.push(chars.next().expect("peeked"));
                for c in chars.by_ref() {
                    cur.push(c);
                    if c == '\'' {
                        break;
                    }
                    delim.push(c);
                }
            }
            Some('"') => {
                cur.push(chars.next().expect("peeked"));
                while let Some(c) = chars.next() {
                    cur.push(c);
                    if c == '"' {
                        break;
                    }
                    if c == '\\' {
                        // Inside double quotes `\` escapes only `"`, `\`, `$`, backtick.
                        if let Some(&n) = chars.peek() {
                            chars.next();
                            cur.push(n);
                            if !matches!(n, '"' | '\\' | '$' | '`') {
                                delim.push('\\');
                            }
                            delim.push(n);
                        }
                    } else {
                        delim.push(c);
                    }
                }
            }
            Some('\\') => {
                cur.push(chars.next().expect("peeked"));
                match chars.peek() {
                    // Backslash-newline is a line continuation, not part of
                    // the word — stop rather than swallow the next line.
                    Some('\n') | None => break,
                    Some(&n) => {
                        chars.next();
                        cur.push(n);
                        delim.push(n);
                    }
                }
            }
            // Unquoted metacharacter ends the word.
            Some(' ' | '\t' | '\n' | ';' | '&' | '|' | '<' | '>' | '(' | ')') | None => break,
            Some(&c) => {
                chars.next();
                cur.push(c);
                delim.push(c);
            }
        }
    }
    if delim.is_empty() {
        None
    } else {
        Some(Heredoc { delim, strip_tabs })
    }
}

/// Split a shell command line into [`Segment`]s at top-level `;`, `&&`,
/// `||`, `|`, `&`, `\n`. Single-pass lexer tracking quotes, backslash
/// escapes, comments, `$(( ))` arithmetic and heredocs, so separators (and
/// `gradlew` mentions) inside any of those are treated as data.
fn split_segments(cmd: &str) -> Vec<Segment> {
    let mut segs: Vec<Segment> = Vec::new();
    let mut cur = String::new();
    let mut chars = cmd.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;
    let mut in_comment = false;
    // Unclosed parens of a `$(( ... ))` arithmetic expansion; 0 = not in one.
    let mut arith_depth: u32 = 0;
    // True when the next char starts a shell word — the only position where
    // `#` opens a comment.
    let mut at_word_start = true;
    // Heredocs opened on the current line, in order; bodies follow the newline.
    let mut pending_heredocs: Vec<Heredoc> = Vec::new();
    while let Some(c) = chars.next() {
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
        if arith_depth > 0 {
            // Verbatim: `<<` here is a bit shift, separators don't split.
            cur.push(c);
            match c {
                '(' => arith_depth += 1,
                ')' => arith_depth -= 1,
                _ => {}
            }
            at_word_start = false;
            continue;
        }
        if in_comment && c != '\n' {
            cur.push(c);
            continue;
        }
        // A heredoc body starts after the newline that ends the opening line
        // and runs verbatim until its delimiter — everything in it is data
        // (a python script, a config), never commands to analyse.
        if c == '\n' {
            in_comment = false;
            let mut tail = String::new();
            for h in std::mem::take(&mut pending_heredocs) {
                let mut line = String::new();
                loop {
                    match chars.next() {
                        Some('\n') => {
                            let done = if h.strip_tabs {
                                line.trim_start_matches('\t') == h.delim
                            } else {
                                line == h.delim
                            };
                            tail.push_str(&line);
                            tail.push('\n');
                            line.clear();
                            if done {
                                break;
                            }
                        }
                        Some(ch) => line.push(ch),
                        None => {
                            tail.push_str(&line);
                            break;
                        }
                    }
                }
            }
            segs.push(Segment {
                text: std::mem::take(&mut cur),
                sep: "\n".to_string(),
                tail,
            });
            at_word_start = true;
            continue;
        }
        match c {
            '#' if at_word_start => {
                in_comment = true;
                cur.push(c);
            }
            '$' if chars.peek() == Some(&'(') => {
                cur.push(c);
                cur.push(chars.next().expect("peeked"));
                if chars.peek() == Some(&'(') {
                    cur.push(chars.next().expect("peeked"));
                    arith_depth = 2;
                }
                at_word_start = false;
            }
            '<' if chars.peek() == Some(&'<') => {
                cur.push(c);
                cur.push(chars.next().expect("peeked"));
                if chars.peek() == Some(&'<') {
                    // `<<<` is a here-string: no body to skip.
                    cur.push(chars.next().expect("peeked"));
                } else if let Some(h) = lex_heredoc_word(&mut chars, &mut cur) {
                    pending_heredocs.push(h);
                }
                at_word_start = false;
            }
            '\'' => {
                in_single = true;
                cur.push(c);
                at_word_start = false;
            }
            '"' => {
                in_double = true;
                cur.push(c);
                at_word_start = false;
            }
            '\\' => {
                cur.push(c);
                if let Some(&n) = chars.peek() {
                    cur.push(n);
                    chars.next();
                }
                at_word_start = false;
            }
            ';' => {
                segs.push(Segment {
                    text: std::mem::take(&mut cur),
                    sep: ";".to_string(),
                    tail: String::new(),
                });
                at_word_start = true;
            }
            '&' => {
                if chars.peek() == Some(&'&') {
                    chars.next();
                    segs.push(Segment {
                        text: std::mem::take(&mut cur),
                        sep: "&&".to_string(),
                        tail: String::new(),
                    });
                    at_word_start = true;
                } else if cur.ends_with('>') || chars.peek() == Some(&'>') {
                    // Redirect form: `2>&1`, `>&2`, `&>file`. Keep `&` literal.
                    cur.push(c);
                    at_word_start = false;
                } else {
                    segs.push(Segment {
                        text: std::mem::take(&mut cur),
                        sep: "&".to_string(),
                        tail: String::new(),
                    });
                    at_word_start = true;
                }
            }
            '|' => {
                let sep = if chars.peek() == Some(&'|') {
                    chars.next();
                    "||"
                } else {
                    "|"
                };
                segs.push(Segment {
                    text: std::mem::take(&mut cur),
                    sep: sep.to_string(),
                    tail: String::new(),
                });
                at_word_start = true;
            }
            _ => {
                cur.push(c);
                at_word_start = matches!(c, ' ' | '\t' | '(');
            }
        }
    }
    segs.push(Segment {
        text: cur,
        sep: String::new(),
        tail: String::new(),
    });
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
        let seg = &segments[i];
        if seg.sep != "|" {
            continue;
        }
        if !segment_invokes_gradlew(seg.text.trim_start()) {
            continue;
        }
        let next = segments[i + 1].text.trim_start();
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
    for seg in &segments {
        match rewrite_segment(&seg.text) {
            Some(new) => {
                out.push_str(&new);
                any = true;
            }
            None => out.push_str(&seg.text),
        }
        out.push_str(&seg.sep);
        out.push_str(&seg.tail);
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

    // Heredoc lexer regressions (PR #29 review) ───────────────────────────────

    /// `text + sep + tail` over all segments must reproduce the input.
    fn reconstruct(cmd: &str) -> String {
        split_segments(cmd)
            .iter()
            .map(|s| format!("{}{}{}", s.text, s.sep, s.tail))
            .collect()
    }

    #[test]
    fn heredoc_body_not_attached_to_piped_command() {
        // Body follows the whole pipeline; it must not make the `tee`
        // segment look like a gradle invocation.
        let cmd = "cat <<EOF | tee /tmp/run.sh\n./gradlew assemble\nEOF";
        assert_eq!(detect_rewrite(cmd), None);
        assert_eq!(reconstruct(cmd), cmd);
    }

    #[test]
    fn rewrites_gradlew_piped_with_heredoc() {
        let cmd = "./gradlew run <<EOF | tee log\ndata\nEOF";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("gw ./gradlew run <<EOF | tee log\ndata\nEOF")
        );
    }

    #[test]
    fn multiple_heredocs_preserve_newlines() {
        // The newline after an intermediate delimiter must survive: `x\nA\ny\nB`.
        let cmd = "./gradlew task <<A <<B\nx\nA\ny\nB";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("gw ./gradlew task <<A <<B\nx\nA\ny\nB")
        );
    }

    #[test]
    fn backslash_escaped_delimiter() {
        // `<<\EOF` quotes the delimiter; the body still ends at `EOF`.
        let cmd = "cat <<\\EOF\n./gradlew x\nEOF\n./gradlew test";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("cat <<\\EOF\n./gradlew x\nEOF\ngw ./gradlew test")
        );
    }

    #[test]
    fn concatenated_quoted_delimiter_parts() {
        // `<<'END'foo` → delimiter `ENDfoo`; a bare `END` line is body data.
        let cmd = "cat <<'END'foo\nEND\nENDfoo\n./gradlew test";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("cat <<'END'foo\nEND\nENDfoo\ngw ./gradlew test")
        );
        // `<<'E''OF'` → delimiter `EOF`.
        let cmd = "cat <<'E''OF'\n./gradlew x\nEOF\n./gradlew test";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("cat <<'E''OF'\n./gradlew x\nEOF\ngw ./gradlew test")
        );
    }

    #[test]
    fn hyphenated_delimiter() {
        let cmd = "cat <<END-OF-DATA\ndata\nEND-OF-DATA\n./gradlew test";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("cat <<END-OF-DATA\ndata\nEND-OF-DATA\ngw ./gradlew test")
        );
    }

    #[test]
    fn trailing_space_does_not_close_heredoc() {
        // `EOF ` (trailing space) is still body data in shell; only the exact
        // `EOF` line closes the heredoc, so the gradlew line is data too.
        let cmd = "cat <<EOF\n./gradlew test\nEOF \nEOF";
        assert_eq!(detect_rewrite(cmd), None);
        assert_eq!(reconstruct(cmd), cmd);
    }

    #[test]
    fn arithmetic_shift_is_not_heredoc() {
        let cmd = "echo $((1 << 4))\n./gradlew build";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("echo $((1 << 4))\ngw ./gradlew build")
        );
    }

    #[test]
    fn heredoc_operator_in_comment_ignored() {
        let cmd = "echo start\n# docs mention <<EOF\n./gradlew test";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("echo start\n# docs mention <<EOF\ngw ./gradlew test")
        );
    }

    #[test]
    fn hash_inside_word_is_not_comment() {
        // `#` not at word start does not open a comment — the heredoc after
        // it is real and its body must be skipped.
        let cmd = "echo a#b <<EOF\n./gradlew x\nEOF";
        assert_eq!(detect_rewrite(cmd), None);
    }

    #[test]
    fn segments_reconstruct_input_verbatim() {
        for cmd in [
            "cat <<EOF | tee /tmp/run.sh\n./gradlew assemble\nEOF",
            "./gradlew task <<A <<B\nx\nA\ny\nB",
            "cat <<\\EOF\nbody\nEOF\necho done",
            "cat <<END-OF-DATA\ndata\nEND-OF-DATA\n./gradlew test",
            "echo $((1 << 4))\n./gradlew build",
            "echo start\n# docs mention <<EOF\n./gradlew test",
            "cat <<-EOF\n\tindented\n\tEOF",
            "grep x <<< \"data\" && ./gradlew build",
            "a; b && c || d | e & f\ng",
        ] {
            assert_eq!(reconstruct(cmd), cmd, "reconstruction differs for {cmd:?}");
        }
    }
}
