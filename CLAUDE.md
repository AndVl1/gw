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
  hook/          # per-agent hook entries (claude/gemini/cursor + detect)
  init/          # multi-agent install/uninstall (json_hook, rules, opencode)
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

Two-stage: `release-plz` opens a version-bump PR from Conventional Commits, then a tag-driven CD pipeline builds the binaries.

```
push to main → release-plz.yml
  → release-plz-pr job: opens/updates "chore: release vX.Y.Z" PR
       (bumps Cargo.toml + Cargo.lock, regenerates CHANGELOG.md from
        commits since last tag)
  → review/squash-merge the PR

after merge → release-plz.yml runs again
  → release-plz-release job: pushes vX.Y.Z tag (using PAT so it triggers
    downstream workflows)

tag push → cd.yml → release.yml
  → builds 4 targets, uploads tarballs + sha256 + checksums
  → softprops/action-gh-release creates GH release with auto-generated notes
  → homebrew job rewrites Formula/gw.rb in AndVl1/homebrew-tap
  → winget-releaser opens update PR in microsoft/winget-pkgs
```

### Cutting a release

Normal path: just merge to main with Conventional Commit messages. The release-plz PR appears automatically and accumulates commits until you merge it. Bump level is computed from commit types (`feat:` → minor, `fix:`/`perf:` → patch, `feat!:`/`BREAKING CHANGE:` → major post-1.0, minor pre-1.0).

```
gh pr list --label "release-plz"   # check current bump candidate
gh pr merge <N> --squash            # ship it
```

### Manual fallback (cargo-release)

If you need to cut a release without going through release-plz (hotfix, weekend ops, GH Actions outage), `cargo-release` is still configured via `release.toml`:

```bash
cargo release patch --execute       # 0.2.4 → 0.2.5
cargo release minor --execute       # 0.2.4 → 0.3.0
cargo release 0.5.0 --execute       # explicit version
```

Bumps `Cargo.toml`+`Cargo.lock`, commits `chore(release): vX.Y.Z`, tags, pushes. Pre-flight checks: `cargo test`, clean tree, on main. Tag push still triggers cd.yml.

Last-resort fallback: bump version in `Cargo.toml` by hand, commit, `git tag v0.2.1 && git push origin v0.2.1`. Or run `release.yml` via `workflow_dispatch` with explicit tag input.

### Required secrets

- `RELEASE_PLZ_TOKEN` — PAT (fine-grained: contents=read+write, pull-requests=read+write on `AndVl1/gw`). Used by release-plz-action both to open PRs and to push the release tag. **Must be a PAT, not GITHUB_TOKEN** — tags pushed by GITHUB_TOKEN don't trigger downstream workflows, so cd.yml would never fire.
- `TAP_GITHUB_TOKEN` — PAT with write to `AndVl1/homebrew-tap`.
- `WINGET_GITHUB_TOKEN` — classic PAT with `public_repo` scope. Used by `vedantmgoyal9/winget-releaser` to fork `microsoft/winget-pkgs` and open update PR.

### Winget bootstrap (one-time)

Action only updates an *existing* manifest. First submission must be manual:

```bash
# On a Windows machine
winget install wingetcreate
wingetcreate new <url-to-zip-asset>
# follow prompts; package id: AndVl1.gw
```

After first manifest lands in `microsoft/winget-pkgs`, every release tag auto-PRs an update.

### Versioning rules

release-plz computes the bump from Conventional Commits in the release PR's range:

- `fix:` / `perf:` only → patch
- any `feat:` → minor
- `feat!:` / `fix!:` / `BREAKING CHANGE:` footer → minor pre-1.0, major post-1.0

If the manual fallback is used, pick the same level by reading `git log v<last>..HEAD --oneline` before running `cargo release <bump>`.

## Commit Convention

Conventional Commits. Required — release-plz reads them.

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
