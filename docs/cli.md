---
title: CLI
description: Firepit CLI Reference
outline: deep
---

# CLI

## Usage

```
fire [OPTIONS] [TASKS]... [VARS]... [-- <ARGS>...]
```

## Arguments

### Tasks

One or more tasks to run. Tasks run in parallel by default, respecting dependencies.

### Vars

Template variables to override. Variable are in "Name=Value" format (e.g. `ENV=prod`, `DEBUG=true`).
The value is interpreted according to the variable's declaration: inferred for a scalar variable, parsed as the declared type for a [typed variable](/configuration#typed-variables) (e.g. `FILES="[a, b]"` for an `array`).

Only a variable declared in `vars`, at the project level or in the task being run, can be
overridden. A name matching no declaration is an error, so that a typo does not pass silently.
The `args` variable is the exception: it needs no declaration, and is an empty string when no argument is given (see [Args](#args)).

### Args

Arguments after `--` are shell-escaped, joined with a space, and assigned to the `args` template variable, so they can be referenced in task commands as `{{ args }}`.

```
fire test -- --nocapture my_test
```

Embed `{{ args }}` without extra quotes in task commands.
This is an alias for setting `args=...`, so specifying both `args=...` and `-- ...` at the same time is an error.
See [Passing Arguments](/configuration#passing-arguments) for details.

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

Also writes the Firepit log to the specified path. The log always goes to the UI
as well, carrying its level and where in Firepit it was made: in CUI mode along
with the output of the tasks, in TUI mode into the pane of the task each record
is about.

### `--log-level <LOG_LEVEL>`

Level of the Firepit log. Options are: `error`, `warn`, `info`, `debug`, `trace`. Default is `warn`.

`error` and `warn` report what a user needs to act on: a task that could not run, a service that did not become ready, a task restarting.
`info` adds what Firepit is doing to each task, including every try of an exec health check and the output of its command.
`debug` and `trace` add the internals of the runner, and the resolved configuration, which carries the environment of each task.

### `--gantt-file <GANTT_FILE>`

Outputs a Gantt chart showing the execution time of each task in [Mermaid](https://mermaid.js.org/) format to the specified path.

### `--tui`

Force TUI mode, even if tty is not detected.

### `--cui`

Force CUI mode, even if tty is detected.

### `--no-log-prefix`

Disable task label prefixes in CUI log output.
This is useful when piping task logs to external tools that expect raw JSON Lines or other unprefixed output.
