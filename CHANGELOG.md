# Changelog

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
