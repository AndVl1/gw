# gw — Project Guide

Rust CLI. Wraps `./gradlew`. Strips noise. Keeps errors/warnings/status. Heartbeat on stderr. Full log on disk.

## Stack

- Rust 2021, edition `2021`, MSRV bound by clippy 1.95 (CI uses `stable`)
- Crates: `clap`, `regex`, `once_cell`, `anyhow`, `serde`/`serde_json`, `chrono`, `ctrlc`, `tempfile`, `libc` (unix)
- Targets: `aarch64-apple-darwin`, `x86_64-apple-darwin`, `aarch64-unknown-linux-gnu`, `x86_64-unknown-linux-gnu`

## Layout

```
src/
  main.rs        # entrypoint, dispatch
  cli.rs         # arg parse, Dispatch enum
  runner.rs      # spawn child, pipe stdout, write stats
  filter.rs      # line classifier (kotlin/java/lint/test/status)
  parser/        # block parsers (failure, stacktrace, summary)
  heartbeat.rs   # periodic stderr ticks
  log_writer.rs  # build-logs/gw-<ts>.log
  summary.rs     # final stderr summary
  stats.rs       # JSONL append at ~/.local/share/gw/runs.jsonl
  gain.rs        # `gw gain` aggregation + output
  hook/          # `gw init` Claude Code settings patcher
  init/          # init helpers
tests/
  integration.rs # end-to-end runner tests
  fixtures/      # canned gradle output
```

## Build / Test

```bash
cargo build --release
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt
```

CI on push/PR: fmt + clippy + test on `ubuntu-latest`, `macos-latest`.

## Release Flow

Tag-driven via `cargo-release` locally + reusable CI workflow.

```
local: cargo release <bump> --execute
  → bumps Cargo.toml + Cargo.lock
  → commits "chore(release): vX.Y.Z"
  → tags vX.Y.Z
  → pushes commit + tag to origin

remote: push of tag v* triggers cd.yml
  → calls reusable release.yml
  → builds 4 targets, uploads tarballs + sha256 + checksums
  → softprops/action-gh-release creates GH release with auto-generated notes
  → homebrew job rewrites Formula/gw.rb in AndVl1/homebrew-tap
```

### Cutting a release

```bash
# Patch bump (0.2.0 → 0.2.1) — bug fixes only
cargo release patch --execute

# Minor bump (0.2.0 → 0.3.0) — new features
cargo release minor --execute

# Specific version
cargo release 0.5.0 --execute

# Dry-run first if unsure (drop --execute)
cargo release patch
```

Pre-flight: `cargo-release` runs `cargo test` + checks branch/clean state. Aborts if dirty or off main.

### Manual fallback

If `cargo-release` is unavailable: bump `Cargo.toml` version manually, commit, tag, push:

```bash
git tag v0.2.1 && git push origin v0.2.1
```

Or trigger `release.yml` via `workflow_dispatch` with explicit tag input.

### Required secrets

- `TAP_GITHUB_TOKEN` — PAT with write to `AndVl1/homebrew-tap`

### Versioning rules (manual judgment, no longer auto)

- `fix:` only → patch
- `feat:` → minor
- Breaking change → minor pre-1.0, major post-1.0

Pick bump level by reading `git log v<last>..HEAD --oneline` before running `cargo release`.

## Commit Convention

Conventional Commits. Required — release-please reads them.

```
<type>(<scope>)?: <subject>

[body]

[footer]
```

### Types

| Type | Bump | When |
|------|------|------|
| `feat` | minor | New user-facing capability |
| `fix` | patch | Bug fix user notices |
| `perf` | patch | Perf improvement |
| `refactor` | none | Internal, no behavior change |
| `test` | none | Tests only |
| `docs` | none | README/comments |
| `chore` | none | Tooling, deps, version bumps |
| `ci` | none | Workflow changes |
| `build` | none | Cargo.toml, build scripts |
| `style` | none | fmt, whitespace |

Breaking change: `feat!:` or `fix!:` or `BREAKING CHANGE:` footer.

### Scopes (optional)

`runner`, `filter`, `parser`, `gain`, `stats`, `hook`, `init`, `cli`, `heartbeat`, `summary`, `log`, `release`, `ci`.

### Subject

- imperative, lowercase, no period
- ≤ 72 chars
- describe *what + why*, not *how*

### Examples

```
feat(gain): add --since flag for time-window aggregation
fix(filter): drop false-positive on `error: unused import` in deps
perf(parser): precompile regex set once via once_cell
refactor(runner): extract record_passthrough helper
docs: document gw gain JSONL schema
ci: switch darwin matrix to macos-latest
chore(deps): bump clap to 4.5
feat!: drop --legacy-mode flag
```

### Body (optional)

Explain motivation, contrast with previous behavior. Wrap at 72.

### Footer (optional)

```
Refs: #12
Closes: #34
BREAKING CHANGE: --legacy-mode removed; use --full instead
```

## Code Style

- `rustfmt` default config. No custom rules.
- Clippy clean at `-D warnings`. No `#[allow(...)]` without reason in code comment.
- Errors: `anyhow::Result` at boundaries, `thiserror` if domain enum needed (none yet).
- No `unwrap()` in non-test code paths. `expect("invariant: ...")` if truly unreachable.
- Stats writes: best-effort. Never fail user run on stats I/O error.
- Match existing module shape before adding files. Prefer extending `filter.rs`/`parser/` over new top-level module.

## Testing

- Unit tests inline `#[cfg(test)] mod tests`.
- Integration in `tests/integration.rs` — spawns `gw` binary, feeds fixture, asserts stdout/stderr.
- Fixtures: `tests/fixtures/*.txt` (raw gradle output).
- Run `cargo test` before any push. CI enforces.

## Stats Schema

`~/.local/share/gw/runs.jsonl` — one JSON per line, append-only. Schema in `src/stats.rs::Run`. Forward-compatible: extra fields ignored on read.

## Don't

- Don't break `gw init` idempotency. Always `.bak` before write.
- Don't change exit code semantics — must mirror wrapped child.
- Don't strip lines mentioning `error`/`warning` token without test fixture covering it.
- Don't add deps without weighing binary size — release profile is `lto=thin`, `strip=true`, every crate matters.
- Don't bypass conventional commits on main — release-please will skip the version bump silently.
