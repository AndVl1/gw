# gw — Gradle output filter for AI agents

Wraps `./gradlew` (and any wrapper around it: `./mainframer ./gradlew`, `ssh host './gradlew ...'`, etc.) to keep agent contexts tiny:

- **Forwards**: errors (Kotlin `e:`, Java `error:`, Lint), failure blocks, test failures with stacktraces, `BUILD SUCCESSFUL` / `BUILD FAILED`.
- **Suppresses**: daemon noise, dependency downloads, `> Task :foo` lines, `> Configure project`, deprecation banners, passing tests.
- **Heartbeat** on stderr every ~3s (`▸ :app:compileKotlin (45s)`) so agents see the build is alive.
- **Final summary** on stderr with task counts, test counts, error/warning totals, full-log path.
- **Full log** preserved at `./build-logs/gw-<timestamp>.log` for drill-down.

## Install

### One-liner (macOS / Linux)

```bash
curl -sSL https://raw.githubusercontent.com/AndVl1/gw/main/scripts/install.sh | sh
gw init                # patch ~/.claude/settings.json (PreToolUse hook)
```

Pin a version or override install dir:

```bash
curl -sSL https://raw.githubusercontent.com/AndVl1/gw/main/scripts/install.sh | sh -s -- --version v0.2.4 --dir /usr/local/bin
```

Default install dir: `$HOME/.local/bin`. Verifies sha256 by default.

### One-liner (Windows)

PowerShell:

```powershell
irm https://raw.githubusercontent.com/AndVl1/gw/main/scripts/install.ps1 | iex
gw init
```

Default install dir: `%LOCALAPPDATA%\Programs\gw` (added to user `PATH` automatically). Restart your shell after first install.

### Homebrew (macOS, Linux)

Works on macOS and Linux with [Linuxbrew](https://docs.brew.sh/Homebrew-on-Linux):

```bash
brew tap AndVl1/tap
brew install gw
gw init
```

Upgrade:

```bash
brew update && brew upgrade gw
```

### winget (Windows)

```powershell
winget install AndVl1.gw
gw init
```

### From source

```bash
cargo install --path .
gw init
```

### Prebuilt binary (manual)

Download from the [latest release](https://github.com/AndVl1/gw/releases/latest):

- macOS: `gw-<version>-{aarch64,x86_64}-apple-darwin.tar.gz`
- Linux: `gw-<version>-{aarch64,x86_64}-unknown-linux-gnu.tar.gz`
- Windows: `gw-<version>-{aarch64,x86_64}-pc-windows-msvc.zip`

Verify and install (Unix):

```bash
shasum -a 256 -c gw-<version>-<target>.tar.gz.sha256
tar -xzf gw-<version>-<target>.tar.gz
sudo mv gw-<version>-<target>/gw /usr/local/bin/
gw init
```

`gw init` is idempotent and creates a `.bak` of any pre-existing settings file before writing.

Project-local install:

```bash
gw init --local        # patch ./.claude/settings.local.json
```

Uninstall:

```bash
gw init --uninstall          # global
gw init --local --uninstall  # project-local
```

## Usage

Once `gw init` is run, Claude Code's `Bash` tool calls get auto-rewritten when the command contains `gradlew`:

```
./gradlew assembleDebug         →  gw ./gradlew assembleDebug
./mainframer ./gradlew test     →  gw ./mainframer ./gradlew test
sudo FOO=bar ./gradlew lint     →  gw sudo FOO=bar ./gradlew lint
ssh host './gradlew test'       →  gw ssh host './gradlew test'
```

You can also invoke `gw` directly:

```bash
gw ./gradlew assembleDebug              # default streaming filter
gw --full ./gradlew assembleDebug       # passthrough, no filter
gw --quiet ./gradlew test               # errors only
gw --warnings ./gradlew build           # also stream warnings
gw --no-heartbeat ./gradlew lint        # no progress lines on stderr
gw --no-log ./gradlew test              # do not write build-logs/ file
```

The exit code matches the wrapped command's exit code.

### Stats (`gw gain`)

Each run is appended as a JSONL record at `~/.local/share/gw/runs.jsonl`. Aggregate
the savings and per-build stats:

```bash
gw gain                          # aggregate all-time
gw gain --since 7d               # last 7 days (s/m/h/d/w units)
gw gain --history                # table of recent runs
gw gain --history --limit 20     # last 20 runs
```

Example output:

```
gw stats (47 runs, last 7d)
─────────────────────────────────────
Lines:   in 142,538 → out 1,247  (99.1% suppressed)
Bytes:   in 9.2M    → out 68.0K  (99.3% saved)
Time:    avg 1m24s/build, 3 failed
Errors:  5, warnings: 142
```

Stats writes are best-effort: a failing stats file never breaks a build.

## Detection rules

The hook rewrites a command iff:
1. After stripping leading `sudo`/`env`/`nice`/`time` and `VAR=value` env prefixes,
2. it does not already start with `gw `,
3. and it contains the token `gradlew` (word-boundary match, so `./mygradlewhatever` is not touched).

Anything else passes through untouched.

## Output format

While the build runs, `gw` writes to stdout only what an agent needs to act on:

- Kotlin errors (`e: /path/Foo.kt:10:5 Unresolved reference: bar`)
- Java errors (`/path/Foo.java:10: error: cannot find symbol`)
- Lint errors (`/path/Foo.kt:10: Error: ... [LintRuleId]`)
- The full `FAILURE: ...` block
- Test failures with their indented stacktraces (up to 30 follow-on lines)
- `BUILD SUCCESSFUL in ...` / `BUILD FAILED in ...`

Heartbeat lines and the final summary go to **stderr** so they don't get mixed into the agent-visible filtered stream.

Final summary example (stderr):

```
─────────────────────────────────────
BUILD SUCCESSFUL
Tasks: 12 executed, 33 up-to-date, 2 from-cache
Tests: 142 passed, 0 failed, 3 skipped
Diagnostics: 0 errors, 7 warnings, 2 deprecations
Full log: ./build-logs/gw-2026-04-27_18-38-26.log
─────────────────────────────────────
```

## Hook contract (for debugging)

```bash
echo '{"tool_name":"Bash","tool_input":{"command":"./gradlew assemble"}}' \
  | gw hook claude
```

prints

```json
{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","permissionDecisionReason":"gw filter (auto-wrap gradlew)","updatedInput":{"command":"gw ./gradlew assemble"}}}
```

For non-matching commands, the hook exits 0 with empty stdout (Claude Code passes the command through unmodified).

You can also test the rewrite logic directly:

```bash
gw rewrite ./mainframer ./gradlew test
# → gw ./mainframer ./gradlew test    (exit 0)

gw rewrite git status
# (no output, exit 1 — meaning no rewrite)
```

## What gets suppressed

| Pattern | Reason |
|---|---|
| `Starting a Gradle Daemon`, `Daemon will be stopped` | daemon lifecycle noise |
| `Download https://...`, `Downloaded https://...` | dependency-download chatter |
| `> Task :foo`, `> Task :foo UP-TO-DATE`, `FROM-CACHE`, `SKIPPED`, `NO-SOURCE` | task progress (counted, surfaced via heartbeat / summary) |
| `> Configure project :foo` | configuration phase noise |
| `Deprecated Gradle features ...` | deprecation banner (counted, summary) |
| `w: ...` Kotlin / `warning:` Java / `Warning:` Lint | counted, summary; pass `--warnings` to stream them |
| `FooTest > bar PASSED` / `SKIPPED` | counted, summary |
| Indented continuations after suppressed lines | leak prevention |

Everything else — including any line you'd want to act on as an agent — is forwarded.

## Project layout

```
src/
├── cli.rs                 # arg parsing + dispatch
├── main.rs                # entry, exit-code plumbing
├── parser/mod.rs          # LineKind enum + classify(line)
├── filter.rs              # stateful Processor (Forward/Suppress + stats)
├── heartbeat.rs           # background thread, 3s silent threshold
├── runner.rs              # spawn child, merge stdout+stderr, dispatch lines
├── log_writer.rs          # ./build-logs/gw-<ts>.log
├── summary.rs             # final stderr block
├── hook/
│   ├── detect.rs          # ENV_PREFIX strip + gradlew regex + idempotency
│   └── claude.rs          # stdin JSON → stdout hookSpecificOutput
└── init/
    ├── consts.rs          # HOOK_COMMAND etc.
    └── settings.rs        # idempotent settings.json patcher with .bak
tests/
├── fixtures/              # raw gradle outputs for end-to-end tests
└── integration.rs         # spawn-the-binary tests
```

## Security notes

- `build-logs/` may contain sensitive build output (signing keys, API tokens leaked by Gradle plugins). The directory is in `.gitignore`; ensure your project's `.gitignore` also excludes it. Use `--no-log` to disable file logging entirely.
- `--log-dir` is treated as fully trusted; do not pass attacker-controlled values.
- The Claude Code hook only prepends `gw ` to commands containing the `gradlew` token. The rewritten command is exactly the user's original command with one extra prefix; no new attack surface beyond what would otherwise execute.

## License

MIT
