# AGENTS.md

Guidance for AI coding agents working in this repository.

## Project Overview

Firepit is a simple task & service runner with a comfortable TUI, written in
Rust — an alternative to npm scripts, make, or docker-compose for managing
development workflows and services.

## Read First

- [ARCHITECTURE.md](ARCHITECTURE.md) — components, design patterns, task
  execution flow
- [CONTRIBUTING.md](CONTRIBUTING.md) — setup, build/test/lint commands, git
  hooks, PR checklist
- `docs/` — user-facing documentation; `examples/` — runnable example
  projects

## Rules for Agents

- **Worktree setup**: after creating a new git worktree, ALWAYS run
  `mise trust && mise install` inside it before anything else. `mise trust`
  is per-directory, so tools from `mise.toml` are unavailable until this
  runs.
- **Before finishing a task**, run the tests — they are NOT covered by git
  hooks:

  ```bash
  cargo test --all --locked
  ```

  Formatting and lint checks (rustfmt, clippy, dprint) run automatically in
  the pre-commit hook and again in CI, so there is no need to run them
  manually every time — just fix whatever the hook or CI reports.
- **Commits** must follow Conventional Commits (enforced by commitlint via
  lefthook). Never bypass git hooks with `--no-verify`.
- **Generated files**: `schema.json` and `docs/schema-body.md` are
  regenerated and auto-committed by CI when configuration structs in
  `src/config.rs` change — no need to regenerate them manually. This does
  not work on pull requests from forks; regenerate them yourself there
  (see [CONTRIBUTING.md](CONTRIBUTING.md#generated-files)).
- **Tests** are fixture-based: add a directory with a `firepit.yml` under
  `tests/fixtures/` instead of embedding config in test code.
- **Supported platforms** are Linux and macOS only — do not add Windows
  support code. Process code (`src/process/`) should be tested in both PTY
  and non-PTY modes.
