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
/// The word follows Bash quote removal: adjacent parts concatenate
/// (`<<'E''OF'` → `EOF`, `<<\EOF` → `EOF`, `<<'END'foo` → `ENDfoo`),
/// backslash-newline is a line continuation (`<<EO\␤F` → `EOF`), `$'...'`
/// (ANSI-C) and `$"..."` (locale) quotes are recognized, an empty quoted
/// word (`<<''`) is a valid delimiter (body ends at the first empty line),
/// and any character except an unquoted shell metacharacter may appear
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
    // Whitespace (and line continuations) may separate operator and word.
    loop {
        match chars.peek() {
            Some(' ' | '\t') => cur.push(chars.next().expect("peeked")),
            Some('\\') => {
                let mut ahead = chars.clone();
                ahead.next();
                if ahead.peek() == Some(&'\n') {
                    cur.push(chars.next().expect("peeked"));
                    cur.push(chars.next().expect("peeked"));
                } else {
                    break;
                }
            }
            _ => break,
        }
    }
    let mut delim = String::new();
    // An empty `delim` alone can't tell `<<` with no word (syntax error,
    // ignore) from `<<''` (valid: body ends at the first empty line).
    let mut has_word = false;
    loop {
        match chars.peek() {
            Some('\'') => {
                has_word = true;
                cur.push(chars.next().expect("peeked"));
                lex_single_quoted(chars, cur, &mut delim);
            }
            Some('"') => {
                has_word = true;
                cur.push(chars.next().expect("peeked"));
                lex_double_quoted(chars, cur, &mut delim);
            }
            Some('$') => {
                // `$'...'` (ANSI-C) and `$"..."` (locale) are quote forms:
                // the `$` is not part of the word. `$(...)` (and `$((...))`)
                // stays in the word literally — heredoc delimiters undergo
                // no expansion, so bash closes the body at the literal
                // `$(...)` line. A plain `$` is a literal word char.
                let mut ahead = chars.clone();
                ahead.next();
                match ahead.peek() {
                    Some('\'') => {
                        has_word = true;
                        cur.push(chars.next().expect("peeked"));
                        cur.push(chars.next().expect("peeked"));
                        lex_ansi_c_quoted(chars, cur, &mut delim);
                    }
                    Some('"') => {
                        has_word = true;
                        cur.push(chars.next().expect("peeked"));
                        cur.push(chars.next().expect("peeked"));
                        lex_double_quoted(chars, cur, &mut delim);
                    }
                    Some('(') => {
                        has_word = true;
                        let d = chars.next().expect("peeked");
                        cur.push(d);
                        delim.push(d);
                        lex_balanced_parens(chars, cur, &mut delim);
                    }
                    _ => {
                        has_word = true;
                        cur.push(chars.next().expect("peeked"));
                        delim.push('$');
                    }
                }
            }
            Some('`') => {
                // Backtick substitution is likewise literal in the word:
                // `` <<`echo EOF` `` closes at the literal backtick line.
                // `\` escapes the next char, so `` \` `` doesn't close.
                has_word = true;
                let b = chars.next().expect("peeked");
                cur.push(b);
                delim.push(b);
                while let Some(c) = chars.next() {
                    cur.push(c);
                    delim.push(c);
                    if c == '\\' {
                        if let Some(&n) = chars.peek() {
                            chars.next();
                            cur.push(n);
                            delim.push(n);
                        }
                    } else if c == '`' {
                        break;
                    }
                }
            }
            Some('\\') => {
                cur.push(chars.next().expect("peeked"));
                match chars.peek() {
                    // Backslash-newline is a line continuation: both chars
                    // vanish and the word continues (`<<EO\␤F` → `EOF`).
                    Some('\n') => {
                        cur.push(chars.next().expect("peeked"));
                    }
                    None => break,
                    Some(&n) => {
                        has_word = true;
                        chars.next();
                        cur.push(n);
                        delim.push(n);
                    }
                }
            }
            // Unquoted metacharacter ends the word.
            Some(' ' | '\t' | '\n' | ';' | '&' | '|' | '<' | '>' | '(' | ')') | None => break,
            Some(&c) => {
                has_word = true;
                chars.next();
                cur.push(c);
                delim.push(c);
            }
        }
    }
    if has_word {
        Some(Heredoc { delim, strip_tabs })
    } else {
        None
    }
}

/// Consume a `(...)` group verbatim (the `$` already consumed, `(` about to
/// follow): everything up to the matching `)` goes into both `cur` and
/// `delim` literally. Nesting is respected, and parens inside `'...'`,
/// `"..."` or after `\` don't count — `$(echo ")")` is one group.
fn lex_balanced_parens(
    chars: &mut std::iter::Peekable<std::str::Chars>,
    cur: &mut String,
    delim: &mut String,
) {
    let mut depth = 0u32;
    let mut in_single = false;
    let mut in_double = false;
    while let Some(c) = chars.next() {
        cur.push(c);
        delim.push(c);
        if in_single {
            if c == '\'' {
                in_single = false;
            }
            continue;
        }
        match c {
            '\\' => {
                if let Some(&n) = chars.peek() {
                    chars.next();
                    cur.push(n);
                    delim.push(n);
                }
            }
            '"' => in_double = !in_double,
            '\'' if !in_double => in_single = true,
            '(' if !in_double => depth += 1,
            ')' if !in_double => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
    }
}

/// Consume a `'...'` part (opening quote already consumed): contents are
/// literal, only `'` closes.
fn lex_single_quoted(
    chars: &mut std::iter::Peekable<std::str::Chars>,
    cur: &mut String,
    delim: &mut String,
) {
    for c in chars.by_ref() {
        cur.push(c);
        if c == '\'' {
            break;
        }
        delim.push(c);
    }
}

/// Consume a `"..."` part (opening quote already consumed). Inside double
/// quotes `\` escapes only `"`, `\`, `$` and backtick; a backslash-newline
/// is a line continuation and both chars vanish.
fn lex_double_quoted(
    chars: &mut std::iter::Peekable<std::str::Chars>,
    cur: &mut String,
    delim: &mut String,
) {
    while let Some(c) = chars.next() {
        cur.push(c);
        if c == '"' {
            break;
        }
        if c == '\\' {
            match chars.peek() {
                Some('\n') => {
                    cur.push(chars.next().expect("peeked"));
                }
                Some(&n) => {
                    chars.next();
                    cur.push(n);
                    if !matches!(n, '"' | '\\' | '$' | '`') {
                        delim.push('\\');
                    }
                    delim.push(n);
                }
                None => {}
            }
        } else {
            delim.push(c);
        }
    }
}

/// Consume a `$'...'` ANSI-C part (opening `$'` already consumed): `\`
/// escape sequences expand per Bash; an unknown escape keeps the backslash.
/// Numeric escapes (`\xHH`, `\nnn`, `\uHHHH`) are kept literally — they
/// virtually never appear in heredoc delimiters and degrading to a
/// non-matching delimiter is safer than mis-decoding one.
fn lex_ansi_c_quoted(
    chars: &mut std::iter::Peekable<std::str::Chars>,
    cur: &mut String,
    delim: &mut String,
) {
    while let Some(c) = chars.next() {
        cur.push(c);
        if c == '\'' {
            break;
        }
        if c == '\\' {
            if let Some(&n) = chars.peek() {
                chars.next();
                cur.push(n);
                match n {
                    'n' => delim.push('\n'),
                    't' => delim.push('\t'),
                    'r' => delim.push('\r'),
                    'a' => delim.push('\x07'),
                    'b' => delim.push('\x08'),
                    'e' | 'E' => delim.push('\x1b'),
                    'f' => delim.push('\x0c'),
                    'v' => delim.push('\x0b'),
                    '\\' | '\'' | '"' | '?' => delim.push(n),
                    _ => {
                        delim.push('\\');
                        delim.push(n);
                    }
                }
            }
        } else {
            delim.push(c);
        }
    }
}

/// Disambiguate `((` at command position: arithmetic command vs nested
/// subshells. Bash re-parses `((...)` as subshells unless the paren that
/// matches the inner `(` is immediately followed by the outer closing `)`
/// (`((1+2))` → arith; `((cmd) && (cmd))` → subshells). `chars` points at
/// the second `(`. Unterminated input (no matching paren) is already a
/// syntax error; prefer arith so a `<<` inside can't open a phantom heredoc.
///
/// The scan is capped: without a bound, pathological runs of `(` make the
/// repeated lookaheads O(n²) over the whole command. No real arithmetic
/// command is hundreds of chars long, so past the cap treat it as
/// subshells — the plain-char path, same as before the arm existed.
fn arith_command_ahead(chars: &std::iter::Peekable<std::str::Chars>) -> bool {
    const LOOKAHEAD_CAP: usize = 512;
    let mut ahead = chars.clone();
    ahead.next(); // the second `(`
    let mut depth = 1u32;
    for c in ahead.by_ref().take(LOOKAHEAD_CAP) {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return ahead.next() == Some(')');
                }
            }
            _ => {}
        }
    }
    // Cap hit (more input left): not arithmetic. EOF within the cap:
    // unterminated, prefer arith.
    ahead.next().is_none()
}

/// What kind of construct an unmatched `(` opened — decides whether `#`
/// right after the matching `)` starts a comment or continues the current
/// word. Only [`ParenKind::Subshell`] grants comment position; every other
/// kind is a word-level construct whose `)` is followed by more of the same
/// word (so a `<<EOF` after `$(cmd)#x` is a real heredoc, not commentary).
///
/// Arithmetic `((...))` / `$((...))` is tracked separately by `arith_depth`
/// because separators and `<<` inside it must not be lexed as shell syntax.
enum ParenKind {
    /// `(cmd)` grouping subshell or `f()` definition: `)` is a
    /// metacharacter, `#` right after it opens a comment.
    Subshell,
    /// `$(cmd)`: expands inside the surrounding word (`$(cmd)#suffix`).
    CmdSubst,
    /// `<(cmd)` / `>(cmd)`: expands to a `/dev/fd/N` path inside the
    /// surrounding word (`<(cmd)#suffix`).
    ProcSubst,
    /// Compound assignment `name=(...)`, `name+=(...)`, `name[i]=(...)` —
    /// standalone or as a `declare`/`local` argument: the parens belong to
    /// the assignment word (`a=(1 2)#x` is the single word `a=(1 2)#x`).
    /// Recognised as a `(` directly after an unquoted, unescaped `=`: in
    /// valid Bash nothing else puts `(` there.
    Assignment,
    /// Extended glob `?(…)`, `*(…)`, `+(…)`, `@(…)`, `!(…)` (`shopt -s
    /// extglob`): the pattern is one word (`@(foo)#x` is the word
    /// `@(foo)#x`). Recognised as a `(` directly after an unquoted,
    /// unescaped operator char. With extglob off the first four are
    /// syntax errors, so treating them as word-level costs nothing; `!(`
    /// is genuinely ambiguous (`!(cmd)` negates a subshell when extglob
    /// is off). The hook cannot see shopt state, so it picks the reading
    /// that can only miss a rewrite, never rewrite heredoc data.
    Extglob,
}

/// One open `(` on the lexer's stack.
struct OpenParen {
    kind: ParenKind,
    /// `brace_depth` of the enclosing context, restored when this paren
    /// closes. Inside `$(…)`, `<(…)`, `>(…)` normal shell syntax resumes even
    /// when the substitution sits inside a `${…}` (`${x:-$(echo # c)}`), so
    /// each substitution starts at brace depth 0.
    outer_brace_depth: u32,
}

impl ParenKind {
    /// Whether `#` immediately after this construct's closing `)` opens a
    /// comment (true) or continues the current word (false).
    fn grants_comment_position(&self) -> bool {
        match self {
            ParenKind::Subshell => true,
            ParenKind::CmdSubst
            | ParenKind::ProcSubst
            | ParenKind::Assignment
            | ParenKind::Extglob => false,
        }
    }
}

/// Split a shell command line into [`Segment`]s at top-level `;`, `&&`,
/// `||`, `|`, `&`, `\n`. Single-pass lexer tracking quotes, backslash
/// escapes, comments, `$(( ))` arithmetic, `${ }` parameter expansions and
/// heredocs, so separators (and `gradlew` mentions) inside any of those are
/// treated as data.
fn split_segments(cmd: &str) -> Vec<Segment> {
    let mut segs: Vec<Segment> = Vec::new();
    let mut cur = String::new();
    let mut chars = cmd.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;
    let mut in_comment = false;
    // Unclosed parens of a `$(( ... ))` arithmetic expansion; 0 = not in one.
    let mut arith_depth: u32 = 0;
    // True when the open arithmetic construct is a `$((...))` expansion
    // (part of a word) rather than a `((...))` command. Read only while
    // `arith_depth > 0`.
    let mut arith_is_expansion = false;
    // Open `(` constructs outside arithmetic, innermost last. Parens may
    // span separators and newlines, so this is never reset at a segment
    // boundary.
    let mut paren_stack: Vec<OpenParen> = Vec::new();
    // The plain (unquoted, unescaped) char consumed by the previous loop
    // iteration, if that char was plain. Tells `name=(` from `(`. Taken
    // (reset) at the top of every iteration, so only the `_` arm below ever
    // sets it — and only a backslash-newline continuation carries it over.
    let mut last_plain: Option<char> = None;
    // Nesting depth of `${...}` parameter expansions in the *current* paren
    // context. Their contents are literal to this lexer: a `(` in `${x:-(}`
    // or `${f%%(*}` is pattern text, not shell grouping, and `#` inside is
    // never a comment — except inside a nested `$(…)`/`<(…)`/`>(…)`, where
    // shell syntax resumes (the depth is saved on `paren_stack` and starts
    // from 0 there). Not reset at newlines (bash allows them inside); an
    // unterminated `${` is a syntax error whose only effect here is
    // suppressed comment detection.
    let mut brace_depth: u32 = 0;
    // True when the next char starts a shell word — the only position where
    // `#` opens a comment.
    let mut at_word_start = true;
    // Heredocs opened on the current line, in order; bodies follow the newline.
    let mut pending_heredocs: Vec<Heredoc> = Vec::new();
    while let Some(c) = chars.next() {
        let prev_plain = last_plain.take();
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
            // After the closing `))` of an arithmetic *command* bash grants
            // comment position (`((x=1))# note`). An arithmetic *expansion*
            // is part of a word, so `$((1+1))#suffix` continues that word.
            at_word_start = arith_depth == 0 && !arith_is_expansion;
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
                                // `<<-`: the closing line may keep or drop
                                // leading tabs — bash accepts both, even for
                                // a delimiter that itself starts with a tab.
                                line == h.delim || line.trim_start_matches('\t') == h.delim
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
            '(' if at_word_start && chars.peek() == Some(&'(') && arith_command_ahead(&chars) => {
                // `(( ... ))` at command position is an arithmetic command:
                // `<<` inside is a shift, same as in `$(( ))`. Guarded by a
                // lookahead because `((cmd) && (cmd))` is nested subshells.
                cur.push(c);
                cur.push(chars.next().expect("peeked"));
                arith_depth = 2;
                arith_is_expansion = false;
                at_word_start = false;
            }
            '$' if chars.peek() == Some(&'{') => {
                cur.push(c);
                cur.push(chars.next().expect("peeked"));
                brace_depth += 1;
                at_word_start = false;
            }
            '}' if brace_depth > 0 => {
                cur.push(c);
                brace_depth -= 1;
                at_word_start = false;
            }
            '$' if chars.peek() == Some(&'(') => {
                cur.push(c);
                cur.push(chars.next().expect("peeked"));
                if chars.peek() == Some(&'(') {
                    cur.push(chars.next().expect("peeked"));
                    arith_depth = 2;
                    arith_is_expansion = true;
                } else {
                    paren_stack.push(OpenParen {
                        kind: ParenKind::CmdSubst,
                        outer_brace_depth: brace_depth,
                    });
                    brace_depth = 0;
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
            '<' | '>' if chars.peek() == Some(&'(') => {
                // Process substitution: `<(cmd)` / `>(cmd)` becomes a path
                // inside the current word, so `#` after its `)` is no comment.
                cur.push(c);
                cur.push(chars.next().expect("peeked"));
                paren_stack.push(OpenParen {
                    kind: ParenKind::ProcSubst,
                    outer_brace_depth: brace_depth,
                });
                brace_depth = 0;
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
                match chars.peek() {
                    // Backslash-newline is a line continuation: the logical
                    // line goes on, so no segment split, no heredoc body
                    // start, and — since both chars vanish in shell — the
                    // word-boundary state is whatever it was before the
                    // backslash (`echo foo \␤# c` — the `#` opens a comment,
                    // `a=\␤(1 2)#x` is still a compound assignment).
                    Some('\n') => {
                        cur.push(chars.next().expect("peeked"));
                        last_plain = prev_plain;
                    }
                    Some(&n) => {
                        cur.push(n);
                        chars.next();
                        at_word_start = false;
                    }
                    None => {
                        at_word_start = false;
                    }
                }
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
                match c {
                    // Inside `${...}` every char is expansion text: parens
                    // don't open or close anything, spaces don't grant
                    // comment position (`${x:-a # b}` prints `a # b`).
                    _ if brace_depth > 0 => at_word_start = false,
                    '(' => {
                        // `$(`, `<(`, `>(` and `((` were consumed by their
                        // own arms. This `(` is part of a compound assignment
                        // (`a=(1 2)`, `a+=(3)`, `declare -a a=(1)`), part of
                        // an extglob pattern (`@(foo)`), or a metacharacter
                        // opening a grouping subshell / `f()` definition. Only
                        // the last grants comment position after `)`.
                        // Comments are allowed inside an assignment's parens
                        // and a subshell, but a pattern is one word.
                        let kind = match prev_plain {
                            Some('=') => ParenKind::Assignment,
                            Some('?' | '*' | '+' | '@' | '!') => ParenKind::Extglob,
                            _ => ParenKind::Subshell,
                        };
                        at_word_start = !matches!(kind, ParenKind::Extglob);
                        // Reached only with `brace_depth == 0` (the guard
                        // above), so the saved depth is always 0 here.
                        paren_stack.push(OpenParen {
                            kind,
                            outer_brace_depth: brace_depth,
                        });
                    }
                    ')' => {
                        // A stray `)` with nothing open (`case` pattern) is
                        // still a metacharacter: `a)# c` is a comment.
                        at_word_start = match paren_stack.pop() {
                            Some(open) => {
                                brace_depth = open.outer_brace_depth;
                                open.kind.grants_comment_position()
                            }
                            None => true,
                        };
                    }
                    ' ' | '\t' => at_word_start = true,
                    _ => at_word_start = false,
                }
                last_plain = Some(c);
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

    // Second review round (PR #29): Bash quote removal + comment boundaries ───

    #[test]
    fn line_continuation_in_delimiter_word() {
        // `<<EO\␤F` — backslash-newline vanishes, delimiter is `EOF`.
        let cmd = "cat <<EO\\\nF\n./gradlew body\nEOF\n./gradlew test";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("cat <<EO\\\nF\n./gradlew body\nEOF\ngw ./gradlew test")
        );
    }

    #[test]
    fn empty_single_quoted_delimiter() {
        // `<<''` is valid: the body ends at the first empty line. The body
        // must stay byte-identical; only the command after it is rewritten.
        let cmd = "cat <<''\n./gradlew body\n\n./gradlew test";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("cat <<''\n./gradlew body\n\ngw ./gradlew test")
        );
    }

    #[test]
    fn empty_double_quoted_delimiter() {
        let cmd = "cat <<\"\"\n./gradlew body\n\n./gradlew test";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("cat <<\"\"\n./gradlew body\n\ngw ./gradlew test")
        );
    }

    #[test]
    fn ansi_c_quoted_delimiter() {
        // `$'EOF'` — the `$` is part of the quoting, not the word.
        let cmd = "cat <<$'EOF'\n./gradlew body\nEOF\n./gradlew test";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("cat <<$'EOF'\n./gradlew body\nEOF\ngw ./gradlew test")
        );
    }

    #[test]
    fn ansi_c_quoted_delimiter_with_escape() {
        // `$'A\tB'` expands to `A<TAB>B`; only that exact line closes the body.
        let cmd = "cat <<$'A\\tB'\nAtB\nA\tB\n./gradlew test";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("cat <<$'A\\tB'\nAtB\nA\tB\ngw ./gradlew test")
        );
    }

    #[test]
    fn locale_quoted_delimiter() {
        // `$"EOF"` — locale quoting, delimiter `EOF`.
        let cmd = "cat <<$\"EOF\"\n./gradlew body\nEOF\n./gradlew test";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("cat <<$\"EOF\"\n./gradlew body\nEOF\ngw ./gradlew test")
        );
    }

    #[test]
    fn plain_dollar_stays_in_delimiter() {
        // Heredoc words undergo no expansion: `<<$EOF` means delimiter `$EOF`.
        let cmd = "cat <<$EOF\n./gradlew body\n$EOF\n./gradlew test";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("cat <<$EOF\n./gradlew body\n$EOF\ngw ./gradlew test")
        );
    }

    #[test]
    fn comment_after_line_continuation() {
        // `echo foo \␤# ...` — the continuation joins the lines, leaving
        // `#` at a word start: it's a comment, not a heredoc opener.
        let cmd = "echo foo \\\n# docs mention <<EOF\n./gradlew test";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("echo foo \\\n# docs mention <<EOF\ngw ./gradlew test")
        );
    }

    #[test]
    fn comment_after_subshell_close() {
        // Bash starts a comment right after `)`.
        let cmd = "(echo ok)# docs mention <<EOF\n./gradlew test";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("(echo ok)# docs mention <<EOF\ngw ./gradlew test")
        );
    }

    #[test]
    fn escaped_char_before_hash_is_not_comment() {
        // `\x#` — the escaped char is a word char, so `#` continues the word
        // and the heredoc after it is real.
        let cmd = "echo \\a# <<EOF\n./gradlew x\nEOF";
        assert_eq!(detect_rewrite(cmd), None);
    }

    // Critic-agent findings (pre-commit adversarial review) ──────────────────

    #[test]
    fn standalone_arithmetic_command_is_not_heredoc() {
        // `(( ... ))` at command position: `<<` is a shift.
        let cmd = "((size = 1 << 20))\n./gradlew test";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("((size = 1 << 20))\ngw ./gradlew test")
        );
    }

    #[test]
    fn command_substitution_delimiter_is_literal() {
        // Heredoc words are not expanded: `<<$(echo EOF)` closes only at the
        // literal `$(echo EOF)` line.
        let cmd = "cat <<$(echo EOF)\nbody\n$(echo EOF)\n./gradlew test";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("cat <<$(echo EOF)\nbody\n$(echo EOF)\ngw ./gradlew test")
        );
        // A bare `$` line is body data, not the delimiter — nothing inside
        // the body may be rewritten.
        let cmd = "cat <<$(echo EOF)\n$\n./gradlew x\n$(echo EOF)\necho done";
        assert_eq!(detect_rewrite(cmd), None);
    }

    #[test]
    fn backtick_delimiter_is_literal() {
        let cmd = "cat <<`echo EOF`\nbody\n`echo EOF`\n./gradlew test";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("cat <<`echo EOF`\nbody\n`echo EOF`\ngw ./gradlew test")
        );
    }

    #[test]
    fn dash_heredoc_with_tab_leading_delimiter() {
        // `<<-` closing line may keep its tabs even when the delimiter
        // itself begins with one.
        let cmd = "cat <<-$'\\tX'\nbody\n\tX\n./gradlew test";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("cat <<-$'\\tX'\nbody\n\tX\ngw ./gradlew test")
        );
    }

    // Critic round 2 findings ─────────────────────────────────────────────────

    #[test]
    fn double_paren_subshells_are_not_arithmetic() {
        // `((cmd) && cmd)` — bash re-parses as nested subshells; the inner
        // gradlew is a real invocation and separators must still split.
        assert_eq!(
            detect_rewrite("((cd app) && ./gradlew test)").as_deref(),
            Some("((cd app) && gw ./gradlew test)")
        );
    }

    #[test]
    fn arithmetic_with_nested_parens_stays_arithmetic() {
        let cmd = "((x = (1 << 2) + 3))\n./gradlew test";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("((x = (1 << 2) + 3))\ngw ./gradlew test")
        );
    }

    #[test]
    fn comment_directly_after_arith_close() {
        // `))# note` — bash grants comment position right after the `))`
        // of an arithmetic *command*.
        let cmd = "((x=1))# note <<EOF\n./gradlew test";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("((x=1))# note <<EOF\ngw ./gradlew test")
        );
        // An arithmetic *expansion* is part of a word: `$((1+1))#` does not
        // open a comment, so the `<<EOF` is a real heredoc and everything
        // after the opening line is its (unterminated) body — data.
        let cmd = "echo $((1+1))# note <<EOF\n./gradlew test";
        assert_eq!(detect_rewrite(cmd), None);
    }

    // Paren context before `#` (issue #30) ────────────────────────────────────

    #[test]
    fn hash_after_command_substitution_continues_word() {
        // `$(cmd)#...` continues the word (bash: `foo# docs mention` is one
        // argument), so `<<EOF` is a real redirect and the body is data.
        let cmd = "echo $(printf foo)# docs mention <<EOF\n./gradlew body\nEOF\n./gradlew after";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("echo $(printf foo)# docs mention <<EOF\n./gradlew body\nEOF\ngw ./gradlew after")
        );
        assert_eq!(reconstruct(cmd), cmd);
    }

    #[test]
    fn hash_after_arith_expansion_continues_word() {
        let cmd = "echo $((1+1))# docs mention <<EOF\n./gradlew body\nEOF\n./gradlew after";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("echo $((1+1))# docs mention <<EOF\n./gradlew body\nEOF\ngw ./gradlew after")
        );
        assert_eq!(reconstruct(cmd), cmd);
    }

    #[test]
    fn hash_after_grouping_subshell_opens_comment() {
        // Control: after a grouping subshell's `)` the `#` really is a
        // comment, so `<<EOF` is commentary and the following lines are
        // live commands (the bare `EOF` line is just a failing command).
        let cmd = "(echo ok)# docs mention <<EOF\n./gradlew body\nEOF\n./gradlew after";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("(echo ok)# docs mention <<EOF\ngw ./gradlew body\nEOF\ngw ./gradlew after")
        );
        assert_eq!(reconstruct(cmd), cmd);
    }

    #[test]
    fn hash_after_subshell_nested_in_substitution_continues_word() {
        // The innermost closed paren decides: the subshell's `)` closes
        // inside the substitution, and the substitution's own `)` keeps
        // the word going — bash prints `ok# tail` as one argument.
        let cmd = "echo $( (echo ok) )# tail <<EOF\n./gradlew body\nEOF\n./gradlew after";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("echo $( (echo ok) )# tail <<EOF\n./gradlew body\nEOF\ngw ./gradlew after")
        );
        assert_eq!(reconstruct(cmd), cmd);
    }

    #[test]
    fn hash_after_input_process_substitution_continues_word() {
        // `<(cmd)` expands to `/dev/fd/N` inside the word: bash prints
        // `/dev/fd/63# docs mention`, the `<<EOF` is a real heredoc.
        let cmd = "echo <(printf foo)# docs mention <<EOF\n./gradlew body\nEOF\n./gradlew after";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("echo <(printf foo)# docs mention <<EOF\n./gradlew body\nEOF\ngw ./gradlew after")
        );
        assert_eq!(reconstruct(cmd), cmd);
    }

    #[test]
    fn hash_after_output_process_substitution_continues_word() {
        let cmd = "echo >(cat)# docs mention <<EOF\n./gradlew body\nEOF\n./gradlew after";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("echo >(cat)# docs mention <<EOF\n./gradlew body\nEOF\ngw ./gradlew after")
        );
        assert_eq!(reconstruct(cmd), cmd);
    }

    #[test]
    fn hash_after_process_substitution_at_command_position() {
        // Same rule when the substitution is the command word itself
        // (bash: `/dev/fd/63#: No such file or directory`, then the body
        // is fed to it as a heredoc).
        let cmd = "<(true)# docs <<EOF\n./gradlew body\nEOF\n./gradlew after";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("<(true)# docs <<EOF\n./gradlew body\nEOF\ngw ./gradlew after")
        );
        assert_eq!(reconstruct(cmd), cmd);
    }

    #[test]
    fn hash_after_compound_assignment_continues_word() {
        // `a=(1 2)#x` is one assignment word (bash then runs `docs` with
        // the heredoc as stdin), so the body is data. Same for `+=`.
        let cmd = "a=(1 2)# docs <<EOF\n./gradlew body\nEOF\n./gradlew after";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("a=(1 2)# docs <<EOF\n./gradlew body\nEOF\ngw ./gradlew after")
        );
        assert_eq!(reconstruct(cmd), cmd);
        let cmd = "a+=(1 2)# docs <<EOF\n./gradlew body\nEOF\n./gradlew after";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("a+=(1 2)# docs <<EOF\n./gradlew body\nEOF\ngw ./gradlew after")
        );
        assert_eq!(reconstruct(cmd), cmd);
    }

    #[test]
    fn comment_inside_compound_assignment() {
        // Comments are allowed between array elements; the `<<EOF` in one
        // is commentary and the next line is a live command.
        let cmd = "a=(1 # docs mention <<EOF\n2)\n./gradlew after";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("a=(1 # docs mention <<EOF\n2)\ngw ./gradlew after")
        );
        assert_eq!(reconstruct(cmd), cmd);
    }

    #[test]
    fn hash_after_function_parens_opens_comment() {
        // `f()` parens are metacharacters even though `(` follows a word
        // char: bash reads `# c <<EOF` as a comment and the next line as
        // the function body.
        let cmd = "f()# c <<EOF\n{ echo in-f; }; f\n./gradlew after";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("f()# c <<EOF\n{ echo in-f; }; f\ngw ./gradlew after")
        );
        assert_eq!(reconstruct(cmd), cmd);
    }

    #[test]
    fn hash_after_case_pattern_opens_comment() {
        // Stray `)` (nothing open): still a metacharacter.
        let cmd = "case a in a)# c <<EOF\n./gradlew body;; esac\nEOF\n./gradlew after";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("case a in a)# c <<EOF\ngw ./gradlew body;; esac\nEOF\ngw ./gradlew after")
        );
        assert_eq!(reconstruct(cmd), cmd);
    }

    #[test]
    fn hash_after_arith_command_in_for_header_opens_comment() {
        let cmd = "for ((i=0;i<1;i++))# c <<EOF\ndo echo loop; done\n./gradlew after";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("for ((i=0;i<1;i++))# c <<EOF\ndo echo loop; done\ngw ./gradlew after")
        );
        assert_eq!(reconstruct(cmd), cmd);
    }

    #[test]
    fn assignment_context_does_not_leak_across_lines() {
        // A line ending in `=` must not make the `(` on the next line look
        // like a compound assignment: it's a grouping subshell.
        let cmd = "x=\n(echo ok)# c <<EOF\n./gradlew after";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("x=\n(echo ok)# c <<EOF\ngw ./gradlew after")
        );
        assert_eq!(reconstruct(cmd), cmd);
    }

    #[test]
    fn assignment_context_survives_line_continuation() {
        // `a=\␤(1 2)#x` — the continuation vanishes, so this is still one
        // compound-assignment word (bash runs `docs` with the heredoc as
        // stdin) and the body is data.
        let cmd = "a=\\\n(1 2)# docs <<EOF\n./gradlew body\nEOF\n./gradlew after";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("a=\\\n(1 2)# docs <<EOF\n./gradlew body\nEOF\ngw ./gradlew after")
        );
        assert_eq!(reconstruct(cmd), cmd);
    }

    #[test]
    fn hash_after_declare_compound_assignment_continues_word() {
        // `declare -a a=(1 2)# docs` — the `(1 2)#` stays inside the
        // assignment argument (bash declares `docs` and reads the heredoc).
        let cmd = "declare -a a=(1 2)# docs <<EOF\n./gradlew body\nEOF\n./gradlew after";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("declare -a a=(1 2)# docs <<EOF\n./gradlew body\nEOF\ngw ./gradlew after")
        );
        assert_eq!(reconstruct(cmd), cmd);
    }

    #[test]
    fn hash_after_case_pattern_ending_in_equals_opens_comment() {
        // `a=)` in a `case` pattern: the `)` closes nothing, `=` before it
        // is irrelevant — bash reads `# c <<EOF` as a comment.
        let cmd = "case a= in a=)# c <<EOF\n./gradlew body;; esac\nEOF\n./gradlew after";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("case a= in a=)# c <<EOF\ngw ./gradlew body;; esac\nEOF\ngw ./gradlew after")
        );
        assert_eq!(reconstruct(cmd), cmd);
    }

    #[test]
    fn hash_after_extglob_pattern_continues_word() {
        // With `shopt -s extglob`, `@(foo)#x` is one word (bash prints
        // `@(foo)# docs` unchanged when nothing matches); the `<<EOF` is a
        // real heredoc and its body is data. Same for every operator.
        for op in ['?', '*', '+', '@', '!'] {
            let cmd = format!("echo {op}(foo)# docs <<EOF\n./gradlew body\nEOF\n./gradlew after");
            assert_eq!(
                detect_rewrite(&cmd).as_deref(),
                Some(
                    format!("echo {op}(foo)# docs <<EOF\n./gradlew body\nEOF\ngw ./gradlew after")
                        .as_str()
                ),
                "operator {op}"
            );
            assert_eq!(reconstruct(&cmd), cmd, "operator {op}");
        }
    }

    #[test]
    fn hash_after_nested_extglob_pattern_continues_word() {
        let cmd = "echo @(a|@(b))# docs <<EOF\n./gradlew body\nEOF\n./gradlew after";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("echo @(a|@(b))# docs <<EOF\n./gradlew body\nEOF\ngw ./gradlew after")
        );
        assert_eq!(reconstruct(cmd), cmd);
    }

    #[test]
    fn hash_after_negated_subshell_with_space_opens_comment() {
        // `! (cmd)` — a space separates the `!` from the `(`, so this is a
        // negated grouping subshell, not an extglob pattern: `#` after `)`
        // is a comment (bash runs the body line and `EOF` as commands).
        let cmd = "! (false)# docs <<EOF\n./gradlew body\nEOF\n./gradlew after";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("! (false)# docs <<EOF\ngw ./gradlew body\nEOF\ngw ./gradlew after")
        );
        assert_eq!(reconstruct(cmd), cmd);
    }

    #[test]
    fn paren_inside_parameter_expansion_does_not_open_subshell() {
        // `${x:-(}` / `${f%%(*}` hold an unbalanced `(` as pattern text. It
        // must not be pushed as a subshell, or the `)` that really closes
        // the enclosing word-level construct would pop it and grant comment
        // position — turning the real `<<EOF` into commentary and exposing
        // the heredoc body to rewriting. Bash: `# c` stays in the word.
        for head in [
            "echo $(echo ${x:-(})",
            "cat <(echo ${x:-(})",
            "a=(${x:-(} b)",
            "n=$(basename ${f%%(*})",
            "echo $(echo ${var//(/[})",
        ] {
            let cmd = format!("{head}# c <<EOF\n./gradlew body\nEOF\n./gradlew after");
            assert_eq!(
                detect_rewrite(&cmd).as_deref(),
                Some(format!("{head}# c <<EOF\n./gradlew body\nEOF\ngw ./gradlew after").as_str()),
                "{head}"
            );
            assert_eq!(reconstruct(&cmd), cmd, "{head}");
        }
    }

    #[test]
    fn paren_inside_parameter_expansion_does_not_close_substitution() {
        // Mirror case: `${x%)}` holds an unbalanced `)`; it must not pop
        // the enclosing `$(`. Bash prints `# c` — the word continues.
        let cmd = "echo $(echo ${x%)})# c <<EOF\n./gradlew body\nEOF\n./gradlew after";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("echo $(echo ${x%)})# c <<EOF\n./gradlew body\nEOF\ngw ./gradlew after")
        );
        assert_eq!(reconstruct(cmd), cmd);
        // And a top-level `${x%)}` must not disturb a later real subshell.
        let cmd = "echo ${x%)}\n(echo ok)# c <<EOF\n./gradlew after";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("echo ${x%)}\n(echo ok)# c <<EOF\ngw ./gradlew after")
        );
        assert_eq!(reconstruct(cmd), cmd);
    }

    #[test]
    fn hash_inside_parameter_expansion_is_not_comment() {
        // Bash prints `a # b# c`: nothing inside `${...}` opens a comment,
        // so the `<<EOF` after the closing brace is a real heredoc.
        let cmd = "echo ${x:-a # b}# c <<EOF\n./gradlew body\nEOF\n./gradlew after";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("echo ${x:-a # b}# c <<EOF\n./gradlew body\nEOF\ngw ./gradlew after")
        );
        assert_eq!(reconstruct(cmd), cmd);
    }

    #[test]
    fn shell_syntax_resumes_inside_substitution_nested_in_parameter_expansion() {
        // `${x:-$(…)}` — inside the `$(…)` bash lexes normally again: `#`
        // opens a comment, so `<<FAKE` is commentary, the body line is a
        // live command (base and 270c391 wrapped it) and the `)}` closes
        // both constructs.
        let cmd = "echo ${x:-$(echo # docs <<FAKE\n./gradlew body\nFAKE\n)}\n./gradlew after";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("echo ${x:-$(echo # docs <<FAKE\ngw ./gradlew body\nFAKE\n)}\ngw ./gradlew after")
        );
        assert_eq!(reconstruct(cmd), cmd);
        // Same for process substitution inside `${…}`.
        let cmd = "cat ${x:-<(echo # c <<FAKE\n./gradlew body\nFAKE\n)}\n./gradlew after";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("cat ${x:-<(echo # c <<FAKE\ngw ./gradlew body\nFAKE\n)}\ngw ./gradlew after")
        );
        assert_eq!(reconstruct(cmd), cmd);
    }

    #[test]
    fn brace_depth_restored_after_nested_substitution_closes() {
        // After the inner `$(…)` closes, the lexer is back inside `${…}`:
        // `}` closes it and `#` right after continues the word (bash prints
        // `a# d`), so the `<<EOF` is a real heredoc.
        let cmd = "echo ${x:-$(echo a # c\n)}# d <<EOF\n./gradlew body\nEOF\n./gradlew after";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("echo ${x:-$(echo a # c\n)}# d <<EOF\n./gradlew body\nEOF\ngw ./gradlew after")
        );
        assert_eq!(reconstruct(cmd), cmd);
        // A `${y%)}` inside the nested `$(…)` is literal there too, and the
        // outer `${…}` is still open when the `$(…)` closes.
        let cmd = "echo ${x:-$(echo ${y%)})}# c <<EOF\n./gradlew body\nEOF\n./gradlew after";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("echo ${x:-$(echo ${y%)})}# c <<EOF\n./gradlew body\nEOF\ngw ./gradlew after")
        );
        assert_eq!(reconstruct(cmd), cmd);
        // A brace group inside the nested `$(…)`: its `}` is a plain char
        // (depth 0 there) and must not close the outer `${`.
        let cmd = "echo ${x:-$( { echo g; } )}# d <<EOF\n./gradlew body\nEOF\n./gradlew after";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("echo ${x:-$( { echo g; } )}# d <<EOF\n./gradlew body\nEOF\ngw ./gradlew after")
        );
        assert_eq!(reconstruct(cmd), cmd);
    }

    #[test]
    fn segments_reconstruct_paren_context_cases() {
        for cmd in [
            "echo $(printf foo)# docs mention <<EOF\n./gradlew body\nEOF\n./gradlew after",
            "echo $((1+1))# docs mention <<EOF\n./gradlew body\nEOF\n./gradlew after",
            "(echo ok)# docs mention <<EOF\n./gradlew body\nEOF\n./gradlew after",
            "echo $( (echo ok) )# tail <<EOF\n./gradlew body\nEOF\n./gradlew after",
            "echo <(printf foo)# docs mention <<EOF\n./gradlew body\nEOF\n./gradlew after",
            "echo >(cat)# docs mention <<EOF\n./gradlew body\nEOF\n./gradlew after",
            "<(true)# docs <<EOF\n./gradlew body\nEOF\n./gradlew after",
            "a=(1 2)# docs <<EOF\n./gradlew body\nEOF\n./gradlew after",
            "a+=(1 2)# docs <<EOF\n./gradlew body\nEOF\n./gradlew after",
            "a=(1 # docs mention <<EOF\n2)\n./gradlew after",
            "f()# c <<EOF\n{ echo in-f; }; f\n./gradlew after",
            "case a in a)# c <<EOF\n./gradlew body;; esac\nEOF\n./gradlew after",
            "for ((i=0;i<1;i++))# c <<EOF\ndo echo loop; done\n./gradlew after",
            "x=\n(echo ok)# c <<EOF\n./gradlew after",
            "a=\\\n(1 2)# docs <<EOF\n./gradlew body\nEOF\n./gradlew after",
            "declare -a a=(1 2)# docs <<EOF\n./gradlew body\nEOF\n./gradlew after",
            "case a= in a=)# c <<EOF\n./gradlew body;; esac\nEOF\n./gradlew after",
            "echo @(foo)# docs <<EOF\n./gradlew body\nEOF\n./gradlew after",
            "echo !(foo)# docs <<EOF\n./gradlew body\nEOF\n./gradlew after",
            "echo @(a|@(b))# docs <<EOF\n./gradlew body\nEOF\n./gradlew after",
            "! (false)# docs <<EOF\n./gradlew body\nEOF\n./gradlew after",
            "echo $(echo ${x:-(})# c <<EOF\n./gradlew body\nEOF\n./gradlew after",
            "a=(${x:-(} b)# c <<EOF\n./gradlew body\nEOF\n./gradlew after",
            "echo $(echo ${x%)})# c <<EOF\n./gradlew body\nEOF\n./gradlew after",
            "echo ${x:-a # b}# c <<EOF\n./gradlew body\nEOF\n./gradlew after",
            "echo ${x:-a\nb}# c <<EOF\n./gradlew body\nEOF\n./gradlew after",
            "echo ${x:-$(echo # docs <<FAKE\n./gradlew body\nFAKE\n)}\n./gradlew after",
            "cat ${x:-<(echo # c <<FAKE\n./gradlew body\nFAKE\n)}\n./gradlew after",
            "echo ${x:-$(echo a # c\n)}# d <<EOF\n./gradlew body\nEOF\n./gradlew after",
            "echo ${x:-$(echo ${y%)})}# c <<EOF\n./gradlew body\nEOF\n./gradlew after",
            "echo ${x:-$( { echo g; } )}# d <<EOF\n./gradlew body\nEOF\n./gradlew after",
            "echo $((1+1))# note <<EOF\n./gradlew test",
        ] {
            assert_eq!(reconstruct(cmd), cmd, "reconstruction differs for {cmd:?}");
        }
    }

    #[test]
    fn quoted_paren_inside_substitution_delimiter() {
        // `$(echo ")")` — the quoted `)` doesn't end the group; the literal
        // `$(echo ")")` line closes the body.
        let cmd = "cat <<$(echo \")\")\nbody\n$(echo \")\")\n./gradlew test";
        assert_eq!(
            detect_rewrite(cmd).as_deref(),
            Some("cat <<$(echo \")\")\nbody\n$(echo \")\")\ngw ./gradlew test")
        );
    }

    #[test]
    fn escaped_backtick_inside_backtick_delimiter() {
        // `` `a\`b` `` — the escaped backtick doesn't close the substitution;
        // the heredoc body never terminates, so nothing is rewritten.
        let cmd = "cat <<`a\\`b`\nbody\n./gradlew test";
        assert_eq!(detect_rewrite(cmd), None);
    }

    #[test]
    fn segments_reconstruct_review_round_two_cases() {
        for cmd in [
            "cat <<EO\\\nF\n./gradlew body\nEOF\n./gradlew test",
            "cat <<''\n./gradlew body\n\n./gradlew test",
            "cat <<\"\"\n./gradlew body\n\n./gradlew test",
            "cat <<$'EOF'\nbody\nEOF\n./gradlew test",
            "cat <<$\"EOF\"\nbody\nEOF\n./gradlew test",
            "cat <<$'A\\tB'\nAtB\nA\tB\n./gradlew test",
            "cat <<$EOF\nbody\n$EOF\n./gradlew test",
            "echo foo \\\n# docs mention <<EOF\n./gradlew test",
            "(echo ok)# docs mention <<EOF\n./gradlew test",
            "cat << \\\nEOF\nbody\nEOF\necho done",
            "((size = 1 << 20))\n./gradlew test",
            "cat <<$(echo EOF)\nbody\n$(echo EOF)\n./gradlew test",
            "cat <<`echo EOF`\nbody\n`echo EOF`\n./gradlew test",
            "cat <<-$'\\tX'\nbody\n\tX\n./gradlew test",
            "((cd app) && ./gradlew test)",
            "((x=1))# note <<EOF\n./gradlew test",
            "cat <<$(echo \")\")\nbody\n$(echo \")\")\n./gradlew test",
            "cat <<`a\\`b`\nbody\n./gradlew test",
        ] {
            assert_eq!(reconstruct(cmd), cmd, "reconstruction differs for {cmd:?}");
        }
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
