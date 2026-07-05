# Contributing to Firepit

This guide covers environment setup, building, testing, and the checks that
must pass before a change can be merged. It is written for both human
contributors and AI agents.

## Prerequisites

- **Rust**: managed by rustup. The toolchain version is pinned in
  `rust-toolchain.toml` (currently `1.91.0`) and is installed automatically
  the first time you run a `cargo` command in this repository.
- **[mise](https://mise.jdx.dev/)**: installs all other development tools
  (Node.js, dprint, lefthook, git-cliff, cargo-dist, commitlint, gitleaks)
  at the versions pinned in `mise.toml`.
- **netcat** (Linux only): some integration tests use `nc` for service
  health checks. On Debian/Ubuntu: `sudo apt install netcat-openbsd`.
  macOS ships with it.

## Setup

Run this once after cloning (and again in every new git worktree — `mise
trust` is per-directory):

```bash
mise trust && mise install
```

`mise install` also installs the git hooks via lefthook (see
[Git hooks](#git-hooks)), so no extra step is needed.

## Building and running

```bash
# Debug build
cargo build

# Release build (same profile as published binaries)
cargo build --profile dist

# Run against an example project
cargo run -- --dir examples/single dev
cargo run -- --dir examples/multi client#dev server#dev

# Watch mode / force TUI
cargo run -- --dir examples/single --watch dev
cargo run -- --dir examples/single --tui dev
```

## Running tests

```bash
# Full test suite (what CI runs, on Ubuntu and macOS)
cargo test --all --locked

# A single integration test target
cargo test --test runner
cargo test --test config
cargo test --test project

# A single test by name
cargo test --test runner -- test_name
```

Notes for writing tests:

- Integration tests live in `tests/` and are fixture-based: each scenario is
  a directory under `tests/fixtures/` containing a `firepit.yml`. Add a new
  fixture directory rather than embedding config in test code.
- Parameterized tests use the `rstest` crate; assertions use `assertables`.
- Cover both single-project and multi-project configurations, and both
  success and failure paths.

## Coding guidelines

- Use `anyhow` for error handling with context.
- Prefer `tokio::spawn` with proper labeling for async tasks; follow Rust
  async best practices with proper cancellation.
- Use channels for inter-component communication (see
  [ARCHITECTURE.md](ARCHITECTURE.md) for the actor pattern).
- Firepit supports Linux and macOS only; there is no need to handle
  Windows-specific behavior such as its signal model.
- Process code in `src/process/` should be tested with both PTY and
  non-PTY modes.

## Formatting and linting

All of these are check-only in CI and the pre-commit hook; run the fix
variants locally before committing:

```bash
# Rust formatting (custom max_width=120, see rustfmt.toml)
cargo fmt --all              # fix
cargo fmt --all --check      # check

# Lints — warnings are errors in CI
cargo clippy --all-targets -- -D warnings

# Non-Rust files (JSON, Markdown, TOML, YAML)
dprint fmt                   # fix
dprint check                 # check
```

## Git hooks

Lefthook (configured in `lefthook.yml`) runs automatically:

- **pre-commit**: `cargo fmt --check`, `cargo clippy -D warnings`,
  `dprint check`, a gitleaks secret scan, and a check that tool versions in
  `mise.toml` are pinned (no `"latest"`).
- **commit-msg**: commitlint enforces
  [Conventional Commits](https://www.conventionalcommits.org/)
  (e.g. `feat: ...`, `fix(runner): ...`). PR titles are checked the same
  way in CI, since PRs are squash-merged.

Do not bypass hooks with `--no-verify`; fix the reported issues instead.

## Generated files

- **`schema.json`** (JSON schema for `firepit.yml`) and
  **`docs/schema-body.md`** are generated. When configuration structs in
  `src/config.rs` change, CI regenerates and auto-commits them on a
  mismatch, so there is no need to regenerate them manually. You can still
  do so locally if you want your PR diff to be complete:

  ```bash
  cargo run --bin firepit-schema
  ./scripts/schema-to-md.js schema.json docs/schema-body.md
  ```

## Common development tasks

### Adding a new task configuration option

1. Update the `TaskConfig` struct in `src/config.rs`
2. Handle the new option in `Task::new()` in `src/project.rs`
3. Add JSON schema annotations and regenerate the schema (see
   [Generated files](#generated-files))
4. Update the example configurations in `examples/`
5. Add tests in `tests/config.rs`

### Modifying UI components

- TUI components live in `src/app/tui/`, CUI components in `src/app/cui/`,
  shared command types in `src/app/command.rs`.
- Follow ratatui patterns for TUI development.

## Documentation site

The docs under `docs/` are a VitePress site published to GitHub Pages:

```bash
cd docs
npm ci
npm run dev      # local dev server
npm run build    # production build (what CI deploys)
```

## Pull request checklist

Before opening a PR, run the tests — they are not covered by git hooks:

```bash
cargo test --all --locked
```

Formatting and lint checks (rustfmt, clippy, dprint, gitleaks) run
automatically in the pre-commit hook, so there is no need to run them
manually before every commit — but keep in mind CI enforces the same
checks, so fix anything the hook or CI reports rather than bypassing it.

- Work on a feature branch; never push directly to `main`.
- Use a Conventional Commits style PR title (commitlint checks it in CI).
- Add or update tests for any behavior change.

## Where to learn more

- `ARCHITECTURE.md` — components, design patterns, and task execution flow.
- `AGENTS.md` — guidance for AI coding agents (`CLAUDE.md` just imports it).
- `docs/` — user-facing documentation (installation, configuration, TUI).
- `examples/` — runnable example projects (`hello`, `single`, `multi`).
