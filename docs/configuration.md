---
title: Configuration
description: Firepit Configuration Guide
outline: deep
---

# Configuration

Firepit is configured with a single YAML file named `firepit.yml` (or `firepit.yaml`) in your project directory.
It defines the tasks you can run, along with their dependencies, variables, environment, and other settings.

The smallest possible configuration is a single task:

```yaml
tasks:
  hello:
    command: echo "hello, firepit"
```

Run it by name:

```bash
fire hello
```

The rest of this page builds up from here: defining [tasks](#tasks), parameterizing them with [variables](#variables) and [environment variables](#environment-variables), wiring them together with [dependencies](#dependencies), running long-lived [services](#services), speeding up reruns with [incremental builds and watch mode](#incremental-builds-and-watch-mode), and—for larger setups—[multi-project monorepos](#multi-project-monorepo) and [shared configuration](#reusing-configuration).

## Tasks

A task is a named command. Tasks are defined under the `tasks` key, where each key is the task name.

```yaml
tasks:
  build:
    command: bun build src/index.ts --outfile dist/app
```

By default a command runs in your shell, in the project directory. You can override both:

- **`shell`:** The shell command used to run the command, and its arguments.
- **`working_dir`:** The directory to run the command in, relative to the project directory (or an absolute path).

```yaml
tasks:
  build:
    command: bun build src/index.ts --outfile dist/app
    working_dir: app
    shell:
      command: bash
      args: ["-c"]
```

Tasks run to completion and exit. Long-running processes such as servers are modeled as [services](#services) instead.

### Description and Label

You can annotate a task with a `description` and a `label`.

- **`description`:** A human-readable explanation of what the task does.
  It is shown in the task listing (`fire --list`) and help output. It may span multiple lines.
- **`label`:** A display name used instead of the task name in the TUI/CUI output (for example as the log prefix and pane title).
  When omitted, the task name is used. Unlike `description`, `label` also supports [template variables](#variables) (covered in the next section).

```yaml
tasks:
  dev:
    description: Start the dev server with hot reload
    label: "{{ project }}/dev"
    command: bun run --hot src/index.ts
    service: true
```

## Variables

You can define template variables using the `vars` field, both at the project level and per task.

```yaml
# Project level variables
vars:
  aws_account_id: 123456789012
  aws_region: ap-northeast-1
  ecr_registry: "{{ aws_account_id }}.dkr.ecr.{{ aws_region }}.amazonaws.com"

tasks:
  build:
    # Task level variables
    vars:
      app_name: single
      ecr_repository: "{{ ecr_registry }}/{{ app_name }}"
    command: docker build -t {{ ecr_repository }}:latest .
```

Firepit performs template processing using [Tera](https://keats.github.io/tera/).
Check the documentation for details about the template syntax.

Set a variable without a value to make it required. Required variables must be
provided explicitly from the CLI or a higher-priority `vars` entry; otherwise,
config rendering fails before the task starts.

```yaml
tasks:
  sops-decrypt:
    vars:
      env:
    command: sops -d ../envs/{{ env }}/*.tfvars
```

Template processing is supported in the following fields:

- `vars`
- `label`
- `command`
- `env`
- `env_files`
- `working_dir`
- `depends_on`

There are also some built-in variables available for use in templates.

| Name           | Type                | Description                                                            |
| -------------- | ------------------- | ---------------------------------------------------------------------- |
| `root_dir`     | string              | The absolute path of the root project dir. Multi-projects only.        |
| `project_dirs` | Map<string, string> | Map of all project names to their absolute paths. Multi-projects only. |
| `project_dir`  | string              | The absolute path of the current project directory.                    |
| `project`      | string              | The project name. Multi-projects only.                                 |
| `task`         | string              | The task name.                                                         |
| `watch`        | boolean             | `true` if running in watch mode, `false` otherwise.                    |

### Passing Arguments

The `args` variable is a convenient convention for forwarding command-line arguments to a task.
Declare it with a default value, reference it in the command, and override it from the CLI using `--`:

```yaml
tasks:
  test:
    vars:
      args: ""
    command: cargo test {{ args }}
```

```
fire test -- --nocapture my_test   # runs: cargo test --nocapture my_test
```

Everything after `--` is shell-escaped, joined with a space, and assigned to `args`.
Embed `{{ args }}` without extra quotes so the shell can interpret the generated quoting correctly.
Since `--` is just an alias for `args=...`, specifying both at the same time is an error.

A dependent task can also set `args`—or any variable—on the task it depends on. See [Parameterized Dependencies](#parameterized-dependencies).

### Dynamic Variables

The variables shown so far are _static_: their values are plain JSON (string, number, boolean, array, or map).
A variable can instead be _dynamic_, taking its value from the output of a command.
Write the variable in object form and specify a `command`:

```yaml
vars:
  git_sha:
    command: git rev-parse --short HEAD
  build_date:
    command: date +%Y%m%d
    # Optional fields
    working_dir: .
    shell:
      command: bash
      args: ["-c"]

tasks:
  build:
    command: docker build -t app:{{ git_sha }} .
```

The command's standard output, trimmed of surrounding whitespace, becomes the variable's value; standard error is ignored.
In addition to `command`, a dynamic variable accepts the optional `shell`, `working_dir`, `env`, and `env_files` fields.

## Environment Variables

Environment variables can be defined in the `env` field.
You can also specify [dotenv](https://github.com/motdotla/dotenv) files in the `env_files` field.
The precedence of environment variables is as follows:

1. Environment variables in the `env` field
2. Environment variables from each dotenv file listed in the `env_files` field.
   If the same environment variable is defined in multiple files, the later file takes precedence.
3. OS environment variables

Note that dependency tasks do not inherit the environment variables of their parent task.

```yaml
# Project level environment variables
env:
  TZ: Asia/Tokyo

# Project level dotenv files
env_files:
  - .env

tasks:
  dev:
    command: bun run --hot src/index.ts
    # Task level environment variables
    env:
      PORT: 3000
      REDIS_URL: redis://localhost:6379
    # Task level dotenv files.
    # .env.local has a higher priority than .env
    env_files:
      - .env.local
      - .env
```

## Dependencies

Tasks can depend on other tasks using the `depends_on` field.
Dependency tasks are executed before the target task.

In this example, `install` and `compile` tasks are executed sequentially before the `build` task.

```yaml
tasks:
  install:
    command: bun install

  compile:
    command: bun build src/index.ts --compile --outfile dist/app
    depends_on:
      - install

  build:
    command: docker build -t single:latest .
    depends_on:
      - compile
```

### Parameterized Dependencies

Writing a dependency in object form lets you override its `vars`.
This means you can define a single generic task and reuse it with different inputs, instead of duplicating near-identical tasks.

In this example, the generic `migrate` task is reused by two tasks with different `database` values:

```yaml
tasks:
  migrate:
    vars:
      database: ""
    command: ./migrate.sh {{ database }}

  setup-app:
    command: echo "app is ready"
    depends_on:
      - task: migrate
        vars:
          database: app # runs: ./migrate.sh app

  setup-analytics:
    command: echo "analytics is ready"
    depends_on:
      - task: migrate
        vars:
          database: analytics # runs: ./migrate.sh analytics
```

Each dependent runs its own variant of `migrate` with the overridden variables.
Note that only variables already declared in the dependency task can be overridden, so `migrate` must declare `database` in its `vars`.
If the same variable is also injected globally via `--` (see [Passing Arguments](#passing-arguments)), the value specified here on the dependency takes precedence.

### Cascading Restarts

In [watch mode](#watch-mode), when a dependency task is re-run, the tasks that depend on it are re-run as well by default.
This cascading behavior can be turned off per dependency by writing the dependency in object form and setting `cascade: false`.
A dependency written as a plain string is equivalent to `cascade: true`.

In this example, `build` is re-run when `install` changes, but **not** when `codegen` is re-run.

```yaml
tasks:
  build:
    command: bun build src/index.ts
    depends_on:
      - install # cascade: true (default)
      - task: codegen
        cascade: false # re-running codegen does not re-run build
```

## Services

Most tasks run to completion and exit. A **service** is a long-running process that stays active until stopped—web servers, databases, file watchers, and the like.
Mark a task as a service by setting `service: true`.

```yaml
tasks:
  dev:
    command: bun run --hot src/index.ts
    service: true
```

When another task depends on a service, the service is started first and kept running while the dependent task runs.

### Readiness

When a service is added to the dependencies, the dependent task runs immediately after the service starts by default.

In this example, the `dev` service may start before the `db` service is ready to accept connections.

```yaml
tasks:
  dev:
    command: bun run --hot src/index.ts
    service: true
    depends_on:
      - install
      - db

  db:
    command: redis-server
    service: true
```

You can configure the `db` service to signal its readiness by using the `healthcheck` field.
There are two ways to define a healthcheck:

- **Command:** Runs a command periodically until it exits with a zero status.
- **Log:** Waits until log message appears that matches the given regex.

Most services become _Ready_ when they start listening on a port, so you can easily check this with the `nc` (netcat) command.
By default, healthcheck command is run every 5 seconds, with a timeout of 5 seconds, and up to 3 retries.

```yaml
db:
  command: redis-server
  service:
    healthcheck:
      command: nc -z localhost 6379
      # Default values
      start_period: 0
      interval: 5
      timeout: 5
      retries: 3
```

Sometimes it is sufficient to wait for a specific log output.
In such cases, you can configure the service to be considered _Ready_ when a log message like `Ready to accept connections tcp` appears.

```yaml
db:
  command: redis-server
  service:
    healthcheck:
      log: Ready to accept connections tcp
```

### Restart Policy

You can control whether a service is restarted when its process exits, using the `restart` field.

| Value          | Description                                                 |
| -------------- | ----------------------------------------------------------- |
| `never`        | Never restart the service. **This is the default.**         |
| `always`       | Always restart the service when it exits.                   |
| `always:N`     | Always restart, up to `N` times.                            |
| `on-failure`   | Restart only when the service exits with a non-zero status. |
| `on-failure:N` | Restart on failure, up to `N` times.                        |

```yaml
tasks:
  db:
    command: redis-server
    service:
      # Restart on failure, up to 5 times
      restart: on-failure:5
      healthcheck:
        log: Ready to accept connections tcp
```

## Incremental Builds and Watch Mode

### Incremental Builds

Firepit can skip tasks if there have been no changes since the last successful run that would produce different outputs.
This is called incremental build.

To enable incremental build, specify the `inputs` and `outputs` fields for each task.
You can use glob patterns to specify multiple files. Check the [globset documentation](https://docs.rs/globset/latest/globset/) for the supported syntax.

```yaml
tasks:
  compile:
    command: bun build src/index.ts --compile --outfile dist/app
    inputs:
      - src/**
    outputs:
      - dist/app
    depends_on:
      - install
```

The task will be skipped if the following conditions are met:

- There is at least one file matching the patterns specified in the `inputs` and `outputs` fields
- All files listed in `inputs` are older than the files listed in `outputs`.

### Watch Mode

In watch mode, Firepit monitors the files specified in the `inputs` field and automatically re-runs the task and dependents when changes are detected.
To enable watch mode, add `-w` or `--watch` option.

```bash
fire -w build
```

## Multi-Project (Monorepo)

Firepit projects can be composed into a monorepo: a root `firepit.yml` plus a `firepit.yml` in each subproject.

```
.
├── firepit.yml
└── packages/
    ├── client/
    │   └── firepit.yml
    └── server/
        └── firepit.yml
```

The root `firepit.yml` declares the subprojects and any common tasks.

```yaml
projects:
  client: packages/client
  server: packages/server

tasks:
  install:
    command: bun install
```

Each `firepit.yml` in subprojects defines its own tasks.

::: code-group

```yaml [packages/client/firepit.yml]
tasks:
  dev:
    command: bun run dev
    depends_on:
      - "#install"
      - server#dev
    service: true
```

```yaml [packages/server/firepit.yml]
tasks:
  dev:
    command: bun run dev
    depends_on:
      - "#install"
    service: true
```

:::

Tasks can be referenced across projects using the form `{project}#{task}`.
Note that the root project name is treated as an empty string, so you can reference root tasks with `#{task}`.

For example, to run client's dev task:

```bash
fire client#dev
```

Move to the client directory and run the dev task directly:

```bash
cd packages/client
fire dev
```

Run client & server dev tasks (because root project does not have dev task)

```bash
fire dev
```

This is how Firepit resolves which task to run:

```mermaid
flowchart LR
    A(["Start"])
    A --> B{"Task is in the form`{project}#{task}`?"}
    B -->|Yes| C["Run the project's task"]
    B -->|No| D{"Current directory?"}
    D -->|Child| E["Run the task of the current project"]
    D -->|Root| F{"Task defined in the root project?"}
    F -->|Yes| G["Run the root project's task"]
    F -->|No| H["Run all subprojects' task with the name"]
```

## Reusing Configuration

As your configuration grows, two features help you avoid repetition: `defaults` applies common settings to many tasks at once, and `includes` merges shared files into your config.

### Defaults

The `defaults` field lets you apply common settings to multiple tasks at once, instead of repeating them in every task.
Each entry has an optional `tasks` selector and the settings to apply.

The `tasks` selector decides which tasks an entry applies to:

- A **string** is treated as a regular expression matched against the task name.
- An **array** is treated as an explicit list of task names.
- If **omitted**, the entry applies to all tasks. (Note that an empty string `""` or empty list `[]` matches nothing.)

An entry can set `shell`, `working_dir`, `vars`, `env`, `env_files`, `depends_on`, `service`, `inputs`, and `outputs`.

```yaml
defaults:
  - tasks: "^(build|test)" # regex: applies to build and test
    depends_on:
      - install
    env:
      NODE_ENV: development
  - tasks: [lint, test] # explicit list
    shell:
      command: bash
      args: ["-c"]

tasks:
  build:
    command: bun run build
  test:
    command: bun test
  lint:
    command: bun run lint
```

When multiple entries match the same task, they are merged in order: scalars (`shell`, `working_dir`, `service`) and map keys (`vars`, `env`) are overridden by later entries, while arrays (`env_files`, `depends_on`, `inputs`, `outputs`) are concatenated.
The merged defaults act as a base layer, so any setting defined directly on the task itself takes precedence.

### Merging Config Files

You can merge multiple configuration files using `includes` field.
Starting from an empty YAML, files specified in `includes` are merged in order, followed by the original `firepit.yml`.

If the field name conflicts, merging strategy depends on the field type.

- number, string, boolean: the later one takes precedence.
- list: the later one is appended to the former one.
- map: merged recursively.

Assume we have the following files:

::: code-group

```yaml [common-vars.yml]
vars:
  aws_account_id: 123456789012
  aws_region: ap-northeast-1
```

```yaml [common-tasks.yml]
tasks:
  install:
    command: bun install
```

:::

::: code-group

```yaml [firepit.yml]
includes:
  - common-vars.yml
  - common-tasks.yml

vars:
  ecr_registry: "{{ aws_account_id }}.dkr.ecr.{{ aws_region }}"

tasks:
  dev:
    command: bun run --hot src/index.ts
    depends_on:
      - install
```

:::

Then, the merged configuration is equivalent to:

```yaml
vars:
  aws_account_id: 123456789012
  aws_region: ap-northeast-1
  ecr_registry: "{{ aws_account_id }}.dkr.ecr.{{ aws_region }}"

tasks:
  install:
    command: bun install

  dev:
    command: bun run --hot src/index.ts
    depends_on:
      - install
```

## Reference

This page covers the configuration you will reach for most often.
For the complete list of every field and its type, see the [Schema](/schema).
