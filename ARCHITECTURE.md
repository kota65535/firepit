# Architecture

Firepit is a simple task & service runner with a comfortable TUI, written in
Rust. It provides an alternative to tools like npm scripts, make, or
docker-compose for managing development workflows and services. The tool
supports both single-project and multi-project setups with dependency
management, file watching, and service health checking.

## Components

1. **Configuration System** (`src/config.rs`, `src/project.rs`)
   - Uses YAML configuration files (`firepit.yml` or `firepit.yaml`)
   - Supports hierarchical project structures with root and child projects
   - Handles template variables, environment variables, and includes
     (rendering is implemented by `ConfigRenderer` in `src/template.rs`)
   - Validates task dependencies and project structure

2. **Task Runner** (`src/runner/`)
   - **Graph-based execution**: Uses petgraph for dependency resolution and
     topological sorting
   - **Concurrent execution**: Configurable concurrency with proper
     dependency ordering
   - **File watching**: Automatic task re-execution when input files change
   - **Service management**: Long-running services with health checks
     (log-line and exec probes in `src/probe.rs`) and restart policies

3. **Process Management** (`src/process/`)
   - Manages child processes with proper signal handling
   - PTY support for interactive applications in TUI mode
   - Process cleanup and graceful shutdown with configurable timeouts

4. **User Interface** (`src/app/`)
   - **TUI Mode** (`src/app/tui/`): Interactive terminal UI using ratatui
     for rich display and interaction
   - **CUI Mode** (`src/app/cui/`): Command-line output for CI/CD and
     non-interactive environments
   - Shared command types in `src/app/command.rs`
   - Real-time output streaming and log management (`src/log.rs`)

Supporting modules: `src/cli.rs` (CLI argument parsing), `src/template.rs`
(Tera-based config rendering), `src/probe.rs` (service health check
probes), `src/app/signal.rs` (signal subscription).

## Key Design Patterns

- **Actor Pattern**: Components communicate via channels (runner, app, file
  watcher)
- **Graph Processing**: Task dependencies form a DAG processed with petgraph
- **Template System**: Uses Tera for variable interpolation in
  configurations
- **Async/Await**: Full async architecture with tokio runtime

## Task Execution Flow

1. **Configuration Loading**: Parse and validate firepit.yml files
2. **Dependency Resolution**: Build DAG and perform topological sort
3. **Concurrent Execution**: Spawn tasks respecting dependencies and
   concurrency limits
4. **Process Management**: Monitor processes, handle signals, manage
   restarts
5. **UI Updates**: Stream output and status updates to TUI/CUI

## Service vs Task Distinction

- **Tasks**: Run once and exit (build, test, install commands)
- **Services**: Run continuously (web servers, databases, file watchers)
- Services have health checks and restart policies (default: `never`)
- Services block dependent tasks until their health check probe succeeds;
  the service process itself keeps running in the background

## File Watching Implementation

- Uses `notify` crate for cross-platform file system events
- Debounced events (300ms) to avoid excessive rebuilds
- Matches changed files against task input patterns
- Triggers task restarts only for relevant changes

## Configuration System

### Project Structure Types

1. **Single Project**: firepit.yml with only `tasks` section
2. **Multi Project**: Root firepit.yml with `projects` section pointing to
   subdirectories

### Key Configuration Concepts

- **Task Dependencies**: Use `depends_on` for sequential execution
- **Services**: Long-running tasks with health checks and restart policies
- **Variables**: Template variables for dynamic configuration
- **Environment**: Environment variables and dotenv file support
- **File Watching**: `inputs` file patterns determine which file changes
  trigger a task re-run in watch mode
- **Incremental Builds**: a non-service task with both `inputs` and
  `outputs` defined is skipped as up-to-date when its outputs are newer
  than its inputs (modification-time comparison)

### Configuration Schema

The project maintains a JSON schema (`schema.json`) for firepit.yml
validation, generated from the configuration structs. When modifying
configuration structures, regenerate it (see
[CONTRIBUTING.md](CONTRIBUTING.md#generated-files)).
