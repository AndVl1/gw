# gw — Gradle output filter for AI agents

Wraps `./gradlew` (and any wrapper around it: `./mainframer ./gradlew`, `ssh host './gradlew ...'`, etc.) to keep agent contexts tiny:

- **Forwards**: errors (Kotlin `e:`, Java `error:`, Lint), failure blocks, test failures with stacktraces, `BUILD SUCCESSFUL` / `BUILD FAILED`.
- **Suppresses**: daemon noise, dependency downloads, `> Task :foo` lines, `> Configure project`, deprecation banners, passing tests.
- **Heartbeat** on stderr every ~3s (`▸ :app:compileKotlin (45s) [12 tasks]`) showing the active task, age, cumulative task progress, and a `— slow` marker once a single task exceeds `GW_HEARTBEAT_SLOW_SECS` (default 60s).
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
gw init --local        # patch ./.claude/settings.local.json (Claude Code default)
```

### Other agents

`gw init` defaults to Claude Code. Pick another agent — or several at once:

```bash
gw init --gemini-cli                       # ~/.gemini/settings.json (BeforeTool hook)
gw init --cursor                           # ~/.cursor/hooks.json (beforeShellExecution)
gw init --opencode                         # ~/.config/opencode/plugin/gw.ts
gw init --agent codex --local              # ./AGENTS.md (rules block)
gw init --claude-code --gemini-cli         # multi-target in one run
gw init --all --local                      # everything supported in this project
```

Per-agent integration mechanism:

| Agent | Mechanism | Default file (global) | Local file |
|---|---|---|---|
| `claude-code` | PreToolUse hook + companion `CLAUDE.md` note | `~/.claude/settings.json` | `.claude/settings.local.json` |
| `gemini-cli` | BeforeTool hook + companion `GEMINI.md` note | `~/.gemini/settings.json` | `.gemini/settings.json` |
| `cursor` | `beforeShellExecution` hook + `AGENTS.md` note | `~/.cursor/hooks.json` | `.cursor/hooks.json` |
| `opencode` | TS plugin (`tool.execute.before`) shelling out to `gw rewrite` | `~/.config/opencode/plugin/gw.ts` | `.opencode/plugin/gw.ts` |
| `codex` | Markdown rules block | `~/.codex/AGENTS.md` | `AGENTS.md` |
| `kilocode` | Markdown rules block | `~/.kilocode/rules/gw.md` | `.kilocode/rules/gw.md` |
| `cline` | Markdown rules block | — (local-only) | `.clinerules` |
| `windsurf` | Markdown rules block | — (local-only) | `.windsurfrules` |
| `antigravity` | Markdown rules block | — (local-only) | `AGENTS.md` |
| `copilot` | Markdown rules block | — (local-only) | `.github/copilot-instructions.md` |

Cursor lacks an in-band rewrite mechanism, so its hook denies the call with a `userMessage` instructing the agent to retry the command prefixed with `gw `. Other hook-based agents auto-rewrite transparently.

Rules-based agents (codex/cline/windsurf/...) get an instructional block delimited by `<!-- gw:begin -->` / `<!-- gw:end -->` markers — only that range is rewritten on update or removed on uninstall, so any surrounding content stays untouched.

Uninstall:

```bash
gw uninstall                               # Claude Code, global
gw uninstall --gemini-cli --cursor         # multiple agents at once
gw uninstall --all --local                 # everything in this project
```

### Audit (`gw doctor`)

Read-only check of every supported agent at both global and local scope — useful when you're not sure whether the hook landed, or whether you're still on the legacy `gw hook claude` command.

```bash
gw doctor
```

```
gw 0.3.0

Claude Code:
  ✓ global installed     /Users/me/.claude/settings.json
  - local  no file       .claude/settings.local.json
Gemini CLI:
  ✗ global not installed /Users/me/.gemini/settings.json
  ...
```

Glyphs: `✓` installed, `⚠` legacy command (re-run `gw init --claude-code` to migrate), `✗` file present but no gw hook, `-` no file.

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
  | gw hook claude-code
```

prints

```json
{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","permissionDecisionReason":"gw filter (auto-wrap gradlew)","updatedInput":{"command":"gw ./gradlew assemble"}}}
```

Per-agent hook subcommands: `gw hook claude-code` (alias: `gw hook claude`), `gw hook gemini-cli`, `gw hook cursor`. For non-matching commands, every hook exits 0 with empty stdout so the command passes through unmodified.

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
├── hook/
│   ├── claude.rs          # Claude Code: hookSpecificOutput envelope
│   ├── gemini.rs          # Gemini CLI: decision/tool_input envelope
│   └── cursor.rs          # Cursor: deny + agentMessage retry
└── init/
    ├── agent.rs           # Agent enum + per-agent paths/kinds
    ├── consts.rs          # hook commands, marker tokens, rule bodies
    ├── settings.rs        # atomic write + .bak rotation + O_NOFOLLOW
    ├── json_hook.rs       # Claude/Gemini/Cursor JSON-settings patcher
    ├── rules.rs           # marker-block install/uninstall for rule files
    └── opencode.rs        # TS plugin emit (tool.execute.before)
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
