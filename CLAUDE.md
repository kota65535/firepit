# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Firepit is a simple task & service runner with a comfortable TUI, written in Rust. It provides an alternative to tools like npm scripts, make, or docker-compose for managing development workflows and services. The tool supports both single-project and multi-project setups with dependency management, file watching, and service health checking.

## Core Architecture

### Main Components

1. **Configuration System** (`src/config.rs`, `src/project.rs`)
   - Uses YAML configuration files (`firepit.yml` or `firepit.yaml`)
   - Supports hierarchical project structures with root and child projects
   - Handles template variables, environment variables, and includes
   - Validates task dependencies and project structure

2. **Task Runner** (`src/runner/`)
   - **Graph-based execution**: Uses petgraph for dependency resolution and topological sorting
   - **Concurrent execution**: Configurable concurrency with proper dependency ordering
   - **File watching**: Automatic task re-execution when input files change
   - **Service management**: Long-running services with health checks and restart policies

3. **Process Management** (`src/process/`)
   - Manages child processes with proper signal handling
   - PTY support for interactive applications in TUI mode
   - Process cleanup and graceful shutdown with configurable timeouts

4. **User Interface** (`src/app/`)
   - **TUI Mode**: Interactive terminal UI using ratatui for rich display and interaction
   - **CUI Mode**: Command-line output for CI/CD and non-interactive environments
   - Real-time output streaming and log management

### Key Design Patterns

- **Actor Pattern**: Components communicate via channels (runner, app, file watcher)
- **Graph Processing**: Task dependencies form a DAG processed with petgraph
- **Template System**: Uses Tera for variable interpolation in configurations
- **Async/Await**: Full async architecture with tokio runtime

## Development Commands

### Essential Commands

```bash
# Build the project
cargo build

# Run tests
cargo test

# Run with example configuration
cargo run -- --dir examples/single dev

# Format code (uses custom max_width=120)
cargo fmt

# Lint code
cargo clippy

# Generate documentation
cargo doc --no-deps --open

# Create release build
cargo build --profile dist
```

### Project-Specific Commands

```bash
# Run firepit on example multi-project
cargo run -- --dir examples/multi client#dev server#dev

# Watch mode for development
cargo run -- --dir examples/single --watch dev

# Force TUI mode
cargo run -- --dir examples/single --tui dev

# Generate JSON schema for firepit.yml
cargo run -- schema > schema.json

# Run integration tests
cargo test --test runner
```

### Testing Patterns

The project uses fixture-based testing extensively:
- Test fixtures in `tests/fixtures/` contain various firepit.yml configurations
- Integration tests in `tests/runner.rs` and `tests/config.rs`
- Use `rstest` for parameterized tests
- Test both single and multi-project configurations

## Configuration System

### Project Structure Types

1. **Single Project**: firepit.yml with only `tasks` section
2. **Multi Project**: Root firepit.yml with `projects` section pointing to subdirectories

### Key Configuration Concepts

- **Task Dependencies**: Use `depends_on` for sequential execution
- **Services**: Long-running tasks with health checks and restart policies  
- **Variables**: Template variables for dynamic configuration
- **Environment**: Environment variables and dotenv file support
- **File Watching**: Input/output file patterns for incremental builds

## Architecture Notes

### Task Execution Flow

1. **Configuration Loading**: Parse and validate firepit.yml files
2. **Dependency Resolution**: Build DAG and perform topological sort
3. **Concurrent Execution**: Spawn tasks respecting dependencies and concurrency limits
4. **Process Management**: Monitor processes, handle signals, manage restarts
5. **UI Updates**: Stream output and status updates to TUI/CUI

### Service vs Task Distinction

- **Tasks**: Run once and exit (build, test, install commands)
- **Services**: Run continuously (web servers, databases, file watchers)
- Services have health checks and restart policies
- Services block dependent tasks until ready

### File Watching Implementation

- Uses `notify` crate for cross-platform file system events
- Debounced events (300ms) to avoid excessive rebuilds
- Matches changed files against task input patterns
- Triggers task restarts only for relevant changes

## Development Guidelines

### Code Organization

- Use `anyhow` for error handling with context
- Prefer `tokio::spawn` with proper labeling for async tasks
- Use channels for inter-component communication
- Follow Rust async best practices with proper cancellation

### Testing Approach

- Create fixture directories for integration tests
- Test both success and failure scenarios
- Use `assertables` crate for better test assertions
- Mock external dependencies where appropriate

### Configuration Schema

The project maintains a JSON schema (`schema.json`) for firepit.yml validation. When modifying configuration structures, ensure schema compatibility and regenerate if needed.

## Common Development Tasks

### Adding New Task Configuration Options

1. Update `TaskConfig` struct in `src/config.rs`
2. Handle the new option in `Task::new()` in `src/project.rs`
3. Add JSON schema annotations
4. Update example configurations
5. Add tests in `tests/config.rs`

### Modifying UI Components

- TUI components in `src/app/tui/`
- CUI components in `src/app/cui/`
- Shared command types in `src/app/command.rs`
- Follow ratatui patterns for TUI development

### Process Management Changes

- Core process logic in `src/process/`
- Handle both Unix and Windows signal differences
- Ensure proper cleanup in all exit scenarios
- Test with both PTY and non-PTY modes