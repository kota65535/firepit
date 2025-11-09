---
title: Schema
description: Firepit Configuration File Schema
outline: deep
---

# Schema

## Project

### concurrency

- **Type:** <code>integer</code>
- **Required:** no
- **Default:** Number of available CPU cores
- **Description:** Task concurrency.
  Valid only in root project config.

```yaml
concurrent: 4
```

### depends_on

- **Type:** <code>[]<a href="#DependsOnConfig">DependsOnConfig</a></code>
- **Required:** no
- **Default:** `[]`
- **Description:** Dependency tasks for all tasks.

### env

- **Type:** <code>Map<string, string></code>
- **Required:** no
- **Default:** `{}`
- **Description:** Environment variables for all tasks.

### env_files

- **Type:** <code>[]string</code>
- **Required:** no
- **Default:** `[]`
- **Description:** Dotenv files for all tasks.
  If environment variable duplicates, the later one wins.

### gantt_file

- **Type:** <code>string</code>
- **Required:** no
- **Description:** Gantt chart output file path.

### includes

- **Type:** <code>[]string</code>
- **Required:** no
- **Default:** `[]`
- **Description:** Additional config files to be included.

### log

- **Type:** <code><a href="#LogConfig">LogConfig</a></code>
- **Required:** no
- **Default:** `{"file":null,"level":"info"}`
- **Description:** Log configuration.
  Valid only in root project config.

### projects

- **Type:** <code>Map<string, string></code>
- **Required:** no
- **Default:** `{}`
- **Description:** Child projects.
  Valid only in root project config.

### shell

- **Type:** <code><a href="#ShellConfig">ShellConfig</a></code>
- **Required:** no
- **Default:** `{"args":["-c"],"command":"bash"}`
- **Description:** Shell configuration for all tasks.

### tasks

- **Type:** <code>Map<string, <a href="#TaskConfig">TaskConfig</a>></code>
- **Required:** no
- **Default:** `{}`
- **Description:** Task definitions

### ui

- **Type:** <code><a href="#UI">UI</a></code>
- **Required:** no
- **Default:** `cui`
- **Description:** UI configuration.
  Valid only in root project config.

### vars

- **Type:** <code>Map<string, any></code>
- **Required:** no
- **Default:** `{}`
- **Description:** Template variables.
  Can be used at `working_dir`, `env`, `env_files`, `depends_on`, `depends_on.{task, vars}` and `tasks`.

### working_dir

- **Type:** <code>string</code>
- **Required:** no
- **Default:** `.`
- **Description:** Working directory for all tasks.

## DependsOnConfig

- **Type:** <code>string | <a href="#DependsOnConfigStruct">DependsOnConfigStruct</a></code>

## DependsOnConfigStruct

### cascade

- **Type:** <code>boolean</code>
- **Required:** no
- **Default:** `true`

### task

- **Type:** <code>string</code>
- **Required:** yes

### vars

- **Type:** <code>Map<string, any></code>
- **Required:** no
- **Default:** `{}`

## ExecProbeConfig

### command

- **Type:** <code>string</code>
- **Required:** yes
- **Description:** Command to run

### env

- **Type:** <code>Map<string, string></code>
- **Required:** no
- **Default:** `{}`
- **Description:** Environment variables.
  Merged with the task's env.

### env_files

- **Type:** <code>[]string</code>
- **Required:** no
- **Default:** `[]`
- **Description:** Dotenv files
  Merged with the task's env.

### interval

- **Type:** <code>integer</code>
- **Required:** no
- **Default:** `5`

### retries

- **Type:** <code>integer</code>
- **Required:** no
- **Default:** `3`

### shell

- **Type:** <code><a href="#ShellConfig">ShellConfig</a></code>
- **Required:** no
- **Description:** Shell configuration

### start_period

- **Type:** <code>integer</code>
- **Required:** no
- **Default:** `0`

### timeout

- **Type:** <code>integer</code>
- **Required:** no
- **Default:** `5`

### working_dir

- **Type:** <code>string</code>
- **Required:** no
- **Description:** Working directory

## HealthCheckConfig

- **Type:** <code><a href="#LogProbeConfig">LogProbeConfig</a> | <a href="#ExecProbeConfig">ExecProbeConfig</a></code>

## LogConfig

### file

- **Type:** <code>string</code>
- **Required:** no
- **Description:** Log file path.

### level

- **Type:** <code>string</code>
- **Required:** no
- **Default:** `info`
- **Description:** Log level: error, warn, info, debug, trace

## LogProbeConfig

### log

- **Type:** <code>string</code>
- **Required:** yes
- **Description:** Log regex pattern to determine the task service is ready

### timeout

- **Type:** <code>integer</code>
- **Required:** no
- **Default:** `20`

## Restart

- **Type:** <code>string</code>
- **Options:** `always`, `on-failure`, `never`

## ServiceConfig

- **Type:** <code>boolean | <a href="#ServiceConfigStruct">ServiceConfigStruct</a></code>

## ServiceConfigStruct

### healthcheck

- **Type:** <code><a href="#HealthCheckConfig">HealthCheckConfig</a></code>
- **Required:** no

### restart

- **Type:** <code><a href="#Restart">Restart</a></code>
- **Required:** no
- **Default:** `never`

## ShellConfig

### args

- **Type:** <code>[]string</code>
- **Required:** no
- **Default:** `["-c"]`
- **Description:** Arguments of the shell command.

### command

- **Type:** <code>string</code>
- **Required:** no
- **Default:** `bash`
- **Description:** Shell command.

## TaskConfig

### command

- **Type:** <code>string</code>
- **Required:** no
- **Description:** Command to run

### depends_on

- **Type:** <code>[]<a href="#DependsOnConfig">DependsOnConfig</a></code>
- **Required:** no
- **Default:** `[]`
- **Description:** Dependency task names

### description

- **Type:** <code>string</code>
- **Required:** no
- **Description:** Description

### env

- **Type:** <code>Map<string, string></code>
- **Required:** no
- **Default:** `{}`
- **Description:** Environment variables.
  Merged with the project's env.

### env_files

- **Type:** <code>[]string</code>
- **Required:** no
- **Default:** `[]`
- **Description:** Dotenv files
  Merged with the project's env.

### inputs

- **Type:** <code>[]string</code>
- **Required:** no
- **Default:** `[]`
- **Description:** Inputs file glob patterns

### label

- **Type:** <code>string</code>
- **Required:** no
- **Description:** Label for display

### outputs

- **Type:** <code>[]string</code>
- **Required:** no
- **Default:** `[]`
- **Description:** Output file glob patterns

### service

- **Type:** <code><a href="#ServiceConfig">ServiceConfig</a></code>
- **Required:** no
- **Description:** Service configuration

### shell

- **Type:** <code><a href="#ShellConfig">ShellConfig</a></code>
- **Required:** no
- **Description:** Shell configuration

### vars

- **Type:** <code>Map<string, any></code>
- **Required:** no
- **Default:** `{}`
- **Description:** Template variables.
  Can be used at `label`, `command`, `working_dir`, `env`, `env_files`, `depends_on`, `depends_on.{task, vars}`,
  `service.healthcheck.log` and `service.healthcheck.exec.{command, working_dir, env, env_files}`

### working_dir

- **Type:** <code>string</code>
- **Required:** no
- **Description:** Working directory

## UI

- **Type:** <code>string</code>
- **Options:** `cui`, `tui`

Process finished with exit code 0
