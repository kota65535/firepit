---
title: CLI
description: Firepit CLI Reference
outline: deep
---

# CLI

## Usage

```
fire [OPTIONS] [TASKS]... [VARS]...
```

## Arguments

### Tasks

One or more tasks to run. Tasks run in parallel by default, respecting dependencies.

### Vars

Template variables to override. Variable are in "Name=Value" format (e.g. `ENV=prod`, `DEBUG=true`)

## Options

### `-d, --dir <DIR>`

Working directory. Default is the same directory as `firepit.yml`.

### `-w, --watch`

Enable watch mode. Automatically re-run tasks when the input files change.

### `-f, --force`

Force the execution of only the specified tasks, ignoring dependencies.

### `--ff, --no-ff`

Enable or disable fail-fast mode.
In fail-fast mode, Firepit stops executing further tasks if any task fails.
It is enabled when CUI mode, and disabled when TUI mode by default.

### `--log-file <LOG_FILE>`

Outputs Firepit debug log file to the specified path.

### `--log-level <LOG_LEVEL>`

Log level of Firepit debug log. Options are: `error`, `warn`, `info`, `debug`, `trace`. Default is `info`.

### `--gantt-file <GANTT_FILE>`

Outputs a Gantt chart showing the execution time of each task in [Mermaid](https://mermaid.js.org/) format to the specified path.

### `--tui`

Force TUI mode, even if tty is not detected.

### `--cui`

Force CUI mode, even if tty is detected.
