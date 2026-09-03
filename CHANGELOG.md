# Changelog

## [0.2.14](https://github.com/AndVl1/gw/compare/v0.2.13...v0.2.14) - 2026-09-03

### Fixed

- *(hook)* track command position and case patterns in the paren lexer
- *(hook)* resume shell lexing inside substitutions nested in ${...}
- *(hook)* treat extglob and ${} parens as word-level, not subshells
- *(hook)* treat process substitution and compound assignment parens as word-level
- *(hook)* distinguish subshell close from substitutions before #
- *(hook)* full Bash quote removal and comment boundaries in heredoc lexer
- *(hook)* parse heredocs with single-pass shell lexer
- *(hook)* do not rewrite gradlew mentioned inside heredoc bodies

## [0.2.13](https://github.com/AndVl1/gw/compare/v0.2.12...v0.2.13) - 2026-06-22

### Added

- *(doctor)* warn about conflicting Bash PreToolUse hooks

### Fixed

- *(init)* insert gw hook at index 0 and migrate legacy on install

### Other

- *(init)* add hook ordering and conflict detection tests

## [0.2.12](https://github.com/AndVl1/gw/compare/v0.2.11...v0.2.12) - 2026-05-17

### Added

- *(cli)* add gw upgrade to re-apply installs and migrate layout
- *(init)* use .claude/rules/gw.md for Claude Code instead of CLAUDE.md
- *(hook)* warn when gradlew is piped through tail/head
- *(init)* warn agents to propagate no-truncate rule to subagents

## [0.2.11](https://github.com/AndVl1/gw/compare/v0.2.10...v0.2.11) - 2026-05-16

### Added

- *(runner)* drop log-path hint from startup banner

## [0.2.10](https://github.com/AndVl1/gw/compare/v0.2.9...v0.2.10) - 2026-05-16

### Added

- *(runner,init)* warn agents not to truncate pre-filtered output
- *(runner)* print startup banner before forwarding output

## [0.2.9](https://github.com/AndVl1/gw/compare/v0.2.8...v0.2.9) - 2026-05-10

### Fixed

- *(summary)* fall back to child exit code when no status line seen
- *(hook)* require gradlew in command position, not as bareword arg

## [0.2.8](https://github.com/AndVl1/gw/compare/v0.2.7...v0.2.8) - 2026-05-06

### Fixed

- *(hook)* wrap each gradlew segment in compound shell commands

## [0.2.7](https://github.com/AndVl1/gw/compare/v0.2.6...v0.2.7) - 2026-05-06

### Added

- *(runner)* handle wrapper-command output corruption

## [0.2.6](https://github.com/AndVl1/gw/compare/v0.2.5...v0.2.6) - 2026-05-01

### Added

- *(release)* add winget-specific archives

### Other

- *(winget)* add manual bootstrap workflow + tolerate first-run failure

## [0.2.5](https://github.com/AndVl1/gw/compare/v0.2.4...v0.2.5) - 2026-04-28

### Added

- *(init)* add gw doctor audit command
- *(heartbeat)* add task progress count and slow-task indicator
- *(init)* support 10 agent targets with multi-target install
- install scripts (sh/ps1), Windows builds, Linuxbrew, winget

### Other

- *(release-plz)* switch to git_only baseline
- release v0.2.4
- add release-plz for auto version-bump PRs
- Merge pull request #5 from AndVl1/feat/install-scripts-winget-linuxbrew

## [0.2.4](https://github.com/AndVl1/gw/compare/v0.2.0...v0.2.4) - 2026-04-28

### Added

- *(init)* support 10 agent targets with multi-target install
- install scripts (sh/ps1), Windows builds, Linuxbrew, winget

### Other

- add release-plz for auto version-bump PRs

## [0.2.1](https://github.com/AndVl1/gw/compare/v0.2.0...v0.2.1) (2026-04-27)


### Bug Fixes

* **heartbeat:** track silence on forwarded output only ([e8e19b3](https://github.com/AndVl1/gw/commit/e8e19b34392ecb02c8200edaa6c31acfc5b1a93a))

## [0.2.0](https://github.com/AndVl1/gw/compare/v0.1.1...v0.2.0) (2026-04-27)


### Features

* **heartbeat:** emit to stdout unconditionally every 30s ([1dd30c5](https://github.com/AndVl1/gw/commit/1dd30c5f2a42159a7c268226921687359edd75ef))

## [0.1.1](https://github.com/AndVl1/gw/compare/v0.1.0...v0.1.1) (2026-04-27)


### Bug Fixes

* **init:** use settings.local.json for --local scope ([078973b](https://github.com/AndVl1/gw/commit/078973b3fd9f7f9cd667fb89fa6ad710e0d8d640))
