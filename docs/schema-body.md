## Project

### concurrency

- **Type:** <code>integer</code>
- **Required:** no
- **Default:** `Number of available CPU cores`
- **Description:** Task concurrency.
Valid only in a root project config.
```yaml
concurrency: 4
```

### depends_on

- **Type:** <code>Array&lt;<a href="#dependsonconfig">DependsOnConfig</a>&gt;</code>
- **Required:** no
- **Default:** `[]`
- **Description:** Dependency tasks for all the project tasks.
```yaml
depends_on:
  - '#install'
```

### env

- **Type:** <code>Map&lt;string, string&gt;</code>
- **Required:** no
- **Default:** `{}`
- **Template:** yes
- **Description:** Environment variables for all the project tasks.
```yaml
env:
  TZ: Asia/Tokyo
```

### env_files

- **Type:** <code>Array&lt;string&gt;</code>
- **Required:** no
- **Default:** `[]`
- **Template:** yes
- **Description:** Dotenv files for all the project tasks.
In case of duplicated environment variables, the latter one takes precedence.
```yaml
env_files:
  - .env
  - .env.local
```

### gantt_file

- **Type:** <code>string</code>
- **Required:** no
- **Template:** no
- **Description:** Gantt chart output file path.
Valid only in a root project config.
```yaml
gantt_file: gantt.svg
```

### includes

- **Type:** <code>Array&lt;string&gt;</code>
- **Required:** no
- **Default:** `[]`
- **Template:** yes
- **Description:** Additional config files to be included.
```yaml
includes:
  - common-vars.yml
  - common-tasks.yml
```

### log

- **Type:** <code><a href="#logconfig">LogConfig</a></code>
- **Required:** no
- **Default:** `{"file":null,"level":"info"}`
- **Description:** Log configuration.
Valid only in a root project config.
```yaml
log:
  level: debug
  file: "{{ root_dir }}/firepit.log"
```

### projects

- **Type:** <code>Map&lt;string, string&gt;</code>
- **Required:** no
- **Default:** `{}`
- **Template:** no
- **Description:** Child projects.
Valid only in a root project config.
```yaml
projects:
  client: packages/client
  server: packages/server
```

### shell

- **Type:** <code><a href="#shellconfig">ShellConfig</a></code>
- **Required:** no
- **Default:** `{"args":["-c"],"command":"bash"}`
- **Description:** Shell configuration for all the project tasks.
```yaml
shell:
  command: "bash"
  args: ["-eux", "-c"]
```

### tasks

- **Type:** <code>Map&lt;string, <a href="#taskconfig">TaskConfig</a>&gt;</code>
- **Required:** no
- **Default:** `{}`
- **Template:** no
- **Description:** Task definitions.

### ui

- **Type:** <code><a href="#ui">UI</a></code>
- **Required:** no
- **Default:** `cui`
- **Description:** UI configuration.
Valid only in a root project config.
```yaml
ui: cui
```

### vars

- **Type:** <code>Map&lt;string, <a href="#varsconfig">VarsConfig</a>&gt;</code>
- **Required:** no
- **Default:** `{}`
- **Template:** yes
- **Description:** Template variables for all the project tasks.
```yaml
vars:
  aws_account_id: 123456789012
  aws_region: ap-northeast-1
  ecr_registry: "{{ aws_account_id }}.dkr.ecr.{{ aws_region }}.amazonaws.com"
```

### working_dir

- **Type:** <code>string</code>
- **Required:** no
- **Default:** `.`
- **Template:** yes
- **Description:** Working directory for all the project tasks.
```yaml
working_dir: src
```

## DependsOnConfig

- **Type:** <code>string | <a href="#dependsonconfigstruct">DependsOnConfigStruct</a></code>
- **Template:** yes

## DependsOnConfigStruct

### cascade

- **Type:** <code>boolean</code>
- **Required:** no
- **Default:** `true`
- **Description:** Whether the task restarts if this dependency task restarts.

### task

- **Type:** <code>string</code>
- **Required:** yes
- **Template:** yes
- **Description:** Dependency task name

### vars

- **Type:** <code>Map&lt;string, <a href="#varsconfig">VarsConfig</a>&gt;</code>
- **Required:** no
- **Default:** `{}`
- **Template:** yes
- **Description:** Variables to override the dependency task vars.

## DynamicVars

### command

- **Type:** <code>string</code>
- **Required:** yes
- **Template:** yes
- **Description:** Command

### env

- **Type:** <code>Map&lt;string, string&gt;</code>
- **Required:** no
- **Default:** `{}`
- **Template:** yes
- **Description:** Environment variables

### env_files

- **Type:** <code>Array&lt;string&gt;</code>
- **Required:** no
- **Default:** `[]`
- **Template:** yes
- **Description:** Dotenv files

### shell

- **Type:** <code><a href="#shellconfig">ShellConfig</a></code>
- **Required:** no
- **Description:** Shell configuration

### working_dir

- **Type:** <code>string</code>
- **Required:** no
- **Template:** yes
- **Description:** Working directory

## ExecProbeConfig

### command

- **Type:** <code>string</code>
- **Required:** yes
- **Template:** yes
- **Description:** Command to check if the service is ready

### env

- **Type:** <code>Map&lt;string, string&gt;</code>
- **Required:** no
- **Default:** `{}`
- **Template:** yes
- **Description:** Environment variables. Merged with the task `env`.

### env_files

- **Type:** <code>Array&lt;string&gt;</code>
- **Required:** no
- **Default:** `[]`
- **Template:** yes
- **Description:** Dotenv files. Merged with the task `env_files`.

### interval

- **Type:** <code>integer</code>
- **Required:** no
- **Default:** `5`
- **Description:** Interval in seconds.
The command will run interval seconds after the task is started,
and then again interval seconds after each previous check completes.

### retries

- **Type:** <code>integer</code>
- **Required:** no
- **Default:** `3`
- **Description:** Number of consecutive readiness-check failures allowed before giving up.

### shell

- **Type:** <code><a href="#shellconfig">ShellConfig</a></code>
- **Required:** no
- **Description:** Shell configuration

### start_period

- **Type:** <code>integer</code>
- **Required:** no
- **Default:** `0`
- **Description:** Initialization period in seconds.
Probe failure during that period will not be counted towards the maximum number of retries.

### timeout

- **Type:** <code>integer</code>
- **Required:** no
- **Default:** `5`
- **Description:** Timeout in seconds

### working_dir

- **Type:** <code>string</code>
- **Required:** no
- **Template:** yes
- **Description:** Working directory

## HealthCheckConfig

- **Type:** <code><a href="#logprobeconfig">LogProbeConfig</a> | <a href="#execprobeconfig">ExecProbeConfig</a></code>

## LogConfig

### file

- **Type:** <code>string</code>
- **Required:** no
- **Template:** no
- **Description:** Log file path.

### level

- **Type:** <code>string</code>
- **Required:** no
- **Default:** `info`
- **Template:** no
- **Description:** Log level. Valid values: error, warn, info, debug, trace

## LogProbeConfig

### log

- **Type:** <code>string</code>
- **Required:** yes
- **Template:** yes
- **Description:** Log regex pattern to determine the task service is ready

### timeout

- **Type:** <code>integer</code>
- **Required:** no
- **Default:** `20`
- **Description:** Timeout in seconds

## Restart

- **Type:** <code>string</code>
- **Options:** `always`, `on-failure`, `never`
- **Template:** no

## ServiceConfig

- **Type:** <code>boolean | <a href="#serviceconfigstruct">ServiceConfigStruct</a></code>

## ServiceConfigStruct

### healthcheck

- **Type:** <code><a href="#healthcheckconfig">HealthCheckConfig</a></code>
- **Required:** no
- **Description:** Readiness probe configuration

### restart

- **Type:** <code><a href="#restart">Restart</a></code>
- **Required:** no
- **Default:** `never`
- **Description:** Restart policy

## ShellConfig

### args

- **Type:** <code>Array&lt;string&gt;</code>
- **Required:** no
- **Default:** `["-c"]`
- **Template:** no
- **Description:** Arguments of the shell command.

### command

- **Type:** <code>string</code>
- **Required:** no
- **Default:** `bash`
- **Template:** no
- **Description:** Shell command.

## TaskConfig

### command

- **Type:** <code>string</code>
- **Required:** no
- **Template:** yes
- **Description:** Command to run

### depends_on

- **Type:** <code>Array&lt;<a href="#dependsonconfig">DependsOnConfig</a>&gt;</code>
- **Required:** no
- **Default:** `[]`
- **Description:** Dependency tasks

### description

- **Type:** <code>string</code>
- **Required:** no
- **Template:** no
- **Description:** Description

### env

- **Type:** <code>Map&lt;string, string&gt;</code>
- **Required:** no
- **Default:** `{}`
- **Template:** yes
- **Description:** Environment variables. Merged with the project `env`.

### env_files

- **Type:** <code>Array&lt;string&gt;</code>
- **Required:** no
- **Default:** `[]`
- **Template:** yes
- **Description:** Dotenv files. Merged with the project `env_files`.

### inputs

- **Type:** <code>Array&lt;string&gt;</code>
- **Required:** no
- **Default:** `[]`
- **Template:** no
- **Description:** Inputs file glob patterns

### label

- **Type:** <code>string</code>
- **Required:** no
- **Template:** yes
- **Description:** Label to display instead of the task name.

### outputs

- **Type:** <code>Array&lt;string&gt;</code>
- **Required:** no
- **Default:** `[]`
- **Template:** no
- **Description:** Output file glob patterns

### service

- **Type:** <code><a href="#serviceconfig">ServiceConfig</a></code>
- **Required:** no
- **Description:** Service configurations

### shell

- **Type:** <code><a href="#shellconfig">ShellConfig</a></code>
- **Required:** no
- **Description:** Shell configuration

### vars

- **Type:** <code>Map&lt;string, <a href="#varsconfig">VarsConfig</a>&gt;</code>
- **Required:** no
- **Default:** `{}`
- **Template:** yes
- **Description:** Template variables. Merged with the project `vars`.
Can be used at `label`, `command`, `working_dir`, `env`, `env_files`, `depends_on`, `depends_on.{task, vars}`,
`service.healthcheck.log` and `service.healthcheck.exec.{command, working_dir, env, env_files}`

### working_dir

- **Type:** <code>string</code>
- **Required:** no
- **Template:** yes
- **Description:** Working directory
```yaml
working_dir: dist
```

## UI

- **Type:** <code>string</code>
- **Options:** `cui`, `tui`
- **Template:** no

## VarsConfig

- **Type:** <code><a href="#dynamicvars">DynamicVars</a> | </code>
- **Description:** Vars config
