# GitHub Configuration

This directory contains GitHub-specific configurations for automated workflows and best practices.

## Files Overview

### Workflows (`.github/workflows/`)

#### `rust.yml`
CI pipeline for the `logic` crate (`logic/`) — the pure, hardware-free
calendar/prayer-time logic split out of the firmware so it can be built,
linted, and unit tested with a normal stable host Rust toolchain, with no
ESP32 device or `esp`/ESP-IDF toolchain required (see [issue #2](https://github.com/ergousha/4.0inch_ESP32_Rust_NamazVakti/issues/2)).
The `namaz-vakti` firmware package at the repo root can only be built with
the `esp` toolchain + ESP-IDF SDK, so it isn't checked here — see
`esp32-build.yml` instead.
- **Cargo check**: Verify the logic crate compiles
- **Rustfmt**: Enforce code formatting
- **Clippy**: Linting and best practices
- **Tests**: Run the logic crate's unit test suite
- **Documentation**: Check doc comments and build docs
- **Build**: Release build verification

Triggers on: `push` to main/develop, `pull_request` to main/develop

#### `esp32-build.yml`
ESP32-specific build verification:
- Builds for ESP32 target
- Checks binary size with cargo-bloat
- Runs on macOS for ARM64 host support

Triggers on: `push` to main/develop, `pull_request` to main/develop

#### `security-audit.yml`
Security vulnerability scanning:
- Uses `rustsec/audit-check-action` to check dependencies
- Scheduled weekly on Mondays

Triggers on: `push` to main/develop, `pull_request` to main/develop, weekly schedule

#### `release.yml`
Automated release creation:
- Creates GitHub releases from git tags
- Generates changelog from commit history
- Triggered when pushing version tags (v*.*.*)

Triggers on: push tags matching `v*.*.*`

### Automation

#### `dependabot.yml`
Automated dependency updates:
- **Cargo**: Updates Rust dependencies weekly on Mondays
- **GitHub Actions**: Updates workflow action versions weekly

Auto-generated PRs with labels `dependencies` and appropriate ecosystem tags.

### Configuration Files

#### `CODEOWNERS`
Defines code ownership and review requirements:
- `eakin` is owner of all files by default
- Required for PR reviews

#### `pull_request_template.md`
Template shown when creating pull requests:
- Enforces consistent PR descriptions
- Includes checklist for code quality

## Setup Instructions

### 1. Branch Protection Rules

Configure on GitHub via **Settings → Branches → Branch protection rules**:

**For `main` branch:**

1. Require a pull request before merging:
   - ✅ Require approvals (1 minimum)
   - ✅ Require status checks to pass before merging:
     - `Cargo check (logic crate)`
     - `Rustfmt (logic crate)`
     - `Clippy (logic crate)`
     - `Tests (logic crate)`
     - `Documentation (logic crate)`
     - `Build (logic crate)`
     - `esp32-build (Build for ESP32)`
     - `security_audit (Security Audit)`
   - ✅ Require branches to be up to date before merging
   - ✅ Require code reviews before merging
   - ✅ Require approval of the most recent reviewable push
   - ✅ Require status checks to pass before merging
   - ✅ Dismiss stale pull request approvals when new commits are pushed
   - ✅ Require conversation resolution before merging

2. Include administrators: Consider whether to enforce on admins

3. Restrictions: Optional - restrict who can push to main

### 2. Enable Dependabot

1. Go to **Settings → Code security & analysis**
2. Enable **Dependabot alerts**
3. Enable **Dependabot security updates**
4. Enable **Dependabot version updates** (uses `dependabot.yml`)

### 3. Configure CODEOWNERS

1. The `.github/CODEOWNERS` file is already created
2. Go to **Settings → Branches → Branch protection rules → main**
3. ✅ Enable "Require code reviews from Code Owners"

### 4. Pull Request Settings

1. Go to **Settings → General**
2. Under "Pull Requests":
   - ✅ Allow auto-merge
   - ✅ Allow squash merging
   - ✅ Allow rebase merging

### 5. Actions Settings

1. Go to **Settings → Actions → General**
2. Under "Workflow permissions":
   - Select "Read and write permissions"
   - ✅ Allow GitHub Actions to create and approve pull requests

## Workflow Triggers

| Workflow | Push | PR | Schedule | Tag |
|----------|------|----|-----------|----|
| rust.yml | ✅ | ✅ | - | - |
| esp32-build.yml | ✅ | ✅ | - | - |
| security-audit.yml | ✅ | ✅ | Weekly | - |
| release.yml | - | - | - | ✅ |

## Local Development

All workflows can be run locally:

```bash
# Format check
cargo fmt --all -- --check

# Clippy
cargo clippy --all-targets --all-features -- -D warnings

# Tests
cargo test --verbose

# Documentation
cargo doc --no-deps --document-private-items

# Build
cargo build --release
```

## Dependabot Configuration

Dependabot creates weekly PRs for:
- Direct and indirect Rust dependencies
- GitHub Actions

PRs are labeled with `dependencies` tag and assigned to `eakin`.

## Security Scanning

- Runs weekly security audits via `rustsec`
- Also checks on all PRs and pushes
- Blocks merging if vulnerabilities are found

## Release Process

To create a release:

```bash
# Tag a new version
git tag -a v1.0.0 -m "Release 1.0.0"
git push origin v1.0.0
```

GitHub Actions will automatically:
- Create a GitHub release
- Generate changelog from commits
- Mark as prerelease if version contains `-`

## Maintenance

- Review Dependabot PRs weekly
- Monitor workflow runs for failures
- Update workflows when GitHub Actions versions update
- Check security audit results regularly
