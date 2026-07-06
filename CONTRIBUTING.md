# Contributing to Namaz Vakti

Thank you for your interest in contributing to this project! Here's how to get started.

## Prerequisites

- Rust toolchain installed via Homebrew or rustup
- ESP Rust toolchain (run `espup install`)
- `ldproxy` linker (`cargo install ldproxy`)

## Development Setup

1. Clone the repository:
   ```bash
   git clone <repository-url>
   cd 4.0inch_ESP32_Rust_NamazVakti
   ```

2. Set up the environment:
   ```bash
   source ~/.espup/export-esp.sh
   export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"
   ```

3. Build the project:
   ```bash
   cargo build
   ```

4. Enable the pre-commit hook (one-time, runs fmt/clippy/test on the fast,
   host-testable `logic/` crate before each commit — see
   [`.githooks/pre-commit`](.githooks/pre-commit)):
   ```bash
   git config core.hooksPath .githooks
   ```
   Skip it for a single commit with `git commit --no-verify` if needed.

## Code Quality Standards

All contributions must meet these requirements:

### Formatting
- Format all code with `rustfmt`:
  ```bash
  cargo fmt --all
  ```

### Linting
- Run `clippy` and fix all warnings:
  ```bash
  cargo clippy --all-targets --all-features -- -D warnings
  ```

### Testing
- The pure, hardware-free logic (calendar math, prayer-time parsing) lives in
  the `logic/` crate and is unit tested with a plain host toolchain — no ESP32
  device or `esp` toolchain needed:
  ```bash
  cd logic && cargo test
  ```

### Documentation
- Document all public items
- Ensure no doc warnings:
  ```bash
  cargo doc --no-deps --document-private-items
  ```

## Pull Request Process

1. Create a feature branch from `main`:
   ```bash
   git checkout -b feature/your-feature-name
   ```

2. Make your changes and ensure all checks pass:
   ```bash
   cargo fmt --all
   cargo clippy --all-targets --all-features -- -D warnings
   cargo test
   cargo build --release
   ```

3. Commit with clear messages:
   ```bash
   git commit -m "feat: description of your changes"
   ```

4. Push and create a pull request against `main`

5. Ensure all CI checks pass (GitHub Actions will run automatically)

## Commit Message Guidelines

Follow conventional commits:
- `feat:` for new features
- `fix:` for bug fixes
- `docs:` for documentation
- `style:` for formatting
- `refactor:` for code restructuring
- `perf:` for performance improvements
- `test:` for test additions
- `chore:` for maintenance tasks
- `ci:` for CI/CD changes

## Branch Protection Rules

The `main` branch is protected and requires:
- PR review approval
- All status checks to pass
- Branch to be up to date with base branch

## Automated Updates

- Dependabot automatically creates PRs for dependency updates
- Automatic security audits run weekly
- All PRs run comprehensive CI checks

## Questions or Issues?

Feel free to open an issue on GitHub for questions or bugs.
