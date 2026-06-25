# Changelog

All notable changes to this project will be documented in this file.

## [0.21.0] - 2026-06-25

### 🚀 Features

- *(cli)* Forward arguments after -- to tasks via the args variable (#311)

### 🐛 Bug Fixes

- *(test)* Disable PTY in test runners to prevent TTY-dependent failures (#300)

### ⚙️ Miscellaneous Tasks

- Add lefthook hooks and CI guardrails (#310)

## [0.20.0] - 2026-05-25

### 🚀 Features

- *(tui)* Add scroll indicators to task list (#294)
- *(config)* Add defaults section for shared task settings (#296)
- *(tui)* Add in-TUI URL hover highlight and click-to-open (#299)
- *(tui)* Word selection on double-click, line selection on triple-click (#297)

### 🐛 Bug Fixes

- *(tui)* Remove DarkGray background from quit dialog (#298)

## [0.19.4] - 2026-02-15

### 🐛 Bug Fixes

- Handle zero-sized terminal dimensions (#259)
- Select the entire wrapped lines by double-click (#260)

## [0.19.3] - 2026-02-13

### 🐛 Bug Fixes

- Handle wide chars in search (#255)
- Find matches on wrapped final line (#258)

### ⚙️ Miscellaneous Tasks

- Add mise tools

## [0.19.2] - 2026-02-04

### 🐛 Bug Fixes

- Disable pty for dynamic vars (#250)

## [0.19.1] - 2026-02-04

### 🐛 Bug Fixes

- Make dynamic vars ignore stderr (#249)
- Make dynamic vars ignore stderr

## [0.19.0] - 2026-02-03

### 🚀 Features

- Dynamic vars (#219)

### 🐛 Bug Fixes

- Stop timeout too long (#245)
- Working_dir inheritance (#246)

## [0.18.0] - 2025-12-05

### 🐛 Bug Fixes

- Error message for invalid config files (#223)
- [**breaking**] Reverse `env_files` precedence (#224)

### ⚙️ Miscellaneous Tasks

- Add license

## [0.17.0] - 2025-12-04

### 🚀 Features

- Add `watch` as a built-in var (#222)

### 📚 Documentation

- Env precedence

## [0.16.1] - 2025-12-04

### 🐛 Bug Fixes

- CLI vars evaluation order (#221)

## [0.16.0] - 2025-12-01

### 🚀 Features

- Log probe output (#218)

### 🐛 Bug Fixes

- [**breaking**] Change `env_files` precedence (#215)

### 🚜 Refactor

- Delete unused codes (#217)

### 📚 Documentation

- Prefer aliases
- Prefer aliases
- Recommend github backend
- Fix type
- Add search box

### ⚙️ Miscellaneous Tasks

- Fix log level (#216)

## [0.15.0] - 2025-11-17

### 🚀 Features

- `fire` alias script (#208)

## [0.14.3] - 2025-11-17

### 🐛 Bug Fixes

- Print summary when not a TTY (#201)

### 📚 Documentation

- Fix type links (#206)

## [0.14.2] - 2025-11-14

### 🐛 Bug Fixes

- Incorrect vars in a variant task (#200)

### 📚 Documentation

- Update schema (#196)
- Fix broken HTML tags (#197)
- Provide llms-full.txt (#198)

## [0.14.1] - 2025-11-11

### 🐛 Bug Fixes

- *(deps)* Update rust crate schemars to v1 (#99)
- *(deps)* Update rust crate console-subscriber to 0.5.0 (#187)
- `restart: on-failure` was treated as `always` (#191)

### 📚 Documentation

- Improve `restart` field autocompletion (#190)
- Initialize VitePress (#192)
- Fix .gitignore
- Build VitePress site
- Setup pages by GitHub Action
- Revert gitignore
- Set base for VitePress
- Migrate to vitepress (#195)
- Fix logo path
- Fix text and tagline
- Update README
- Fix tagline
- Add favicon

## [0.14.0] - 2025-11-06

### 🚀 Features

- Reloadable env files (#188)

### 📚 Documentation

- Fix toc

### 🎨 Styling

- Simplify var name

### ⚙️ Miscellaneous Tasks

- Update rust to v1.91.0 (#189)

## [0.13.1] - 2025-10-29

### 🐛 Bug Fixes

- Watch mode was always enabled (#185)

### 📚 Documentation

- Add README (#183)
- Update images
- Supported OS
- Watch mode

## [0.13.0] - 2025-10-27

### 🚀 Features

- Task description (#181)

## [0.12.5] - 2025-10-21

### 🐛 Bug Fixes

- Duplicate variant tasks (#180)

## [0.12.4] - 2025-10-21

### 🐛 Bug Fixes

- Arg vars should override target task vars (#179)

## [0.12.3] - 2025-10-20

### 🐛 Bug Fixes

- Arg vars should be project-level context (#177)

## [0.12.2] - 2025-10-19

### 🐛 Bug Fixes

- Load env files correctly (#175)
- Change "not ready" status icon (#176)

## [0.12.1] - 2025-10-19

### 🐛 Bug Fixes

- Enable scrolling on selection (#174)

## [0.12.0] - 2025-10-15

### 🚀 Features

- [**breaking**] Remove --env option (#171)
- [**breaking**] Pass variables via args instead of -v option (#172)
- Support vars types (#173)

## [0.11.1] - 2025-10-14

### 🐛 Bug Fixes

- Last line is not visible (#168)
- Status summary (#169)

## [0.11.0] - 2025-10-10

### 🚀 Features

- Conditional deps (#165)

## [0.10.0] - 2025-10-09

### 🚀 Features

- Task timeline gantt chart (#160)

### 🐛 Bug Fixes

- Show elapsed time (#159)

## [0.9.3] - 2025-10-08

### 🐛 Bug Fixes

- Failed task name (#158)

## [0.9.2] - 2025-10-06

### 🐛 Bug Fixes

- Show the first failed task (#157)

## [0.9.1] - 2025-10-06

### 🚜 Refactor

- Mouse events (#156)

## [0.9.0] - 2025-09-30

### 🚀 Features

- Fail-fast mode (#154)

### 🐛 Bug Fixes

- Adjust colors and spaces (#152)

### 📚 Documentation

- Fix help (#153)

## [0.8.1] - 2025-09-26

### 🐛 Bug Fixes

- Improve footer layout (#151)

## [0.8.0] - 2025-09-26

### 🚀 Features

- Disable implicit inheritance of root config (#150)

## [0.7.1] - 2025-09-25

### 🐛 Bug Fixes

- Reverse persisted log order (#148)
- Force quit (#149)

### 📚 Documentation

- Force quit

## [0.7.0] - 2025-09-24

### 🚀 Features

- Show status summary (#144)
- Help dialog (#145)
- Force quit (#146)

### 🐛 Bug Fixes

- *(deps)* Update rust crate anyhow to v1.0.100 (#140)
- *(deps)* Update rust crate clap to v4.5.48 (#141)
- *(deps)* Update rust crate serde to v1.0.226 (#142)
- *(deps)* Update rust crate libc to v0.2.176 (#143)

## [0.6.3] - 2025-09-18

### 🐛 Bug Fixes

- *(deps)* Update rust crate console to v0.16.1 (#130)
- *(deps)* Update rust crate serde to v1.0.221 (#131)
- *(deps)* Update rust crate serde_json to v1.0.144 (#132)
- *(deps)* Update rust crate serde_json to v1.0.145 (#133)
- *(deps)* Update rust crate serde to v1.0.223 (#134)
- *(deps)* Update rust crate serde to v1.0.224 (#135)
- *(deps)* Update rust crate serde to v1.0.225 (#136)
- Clearly visible finish line (#138)

## [0.6.2] - 2025-09-04

### 🐛 Bug Fixes

- Preserve env/vars order (#129)

## [0.6.1] - 2025-09-03

### 🐛 Bug Fixes

- Config log

## [0.6.0] - 2025-09-03

### 🚀 Features

- Project level `depends_on` (#128)

### 🐛 Bug Fixes

- Ignore if env file not found (#127)

## [0.5.1] - 2025-09-03

### 🐛 Bug Fixes

- *(deps)* Update rust crate clap to v4.5.42 (#110)
- *(deps)* Update rust crate serde_json to v1.0.142 (#111)
- *(deps)* Update rust crate tokio to v1.47.1 (#112)
- *(deps)* Update rust crate clap to v4.5.43 (#114)
- *(deps)* Update rust crate libc to v0.2.175 (#115)
- *(deps)* Update rust crate clap to v4.5.44 (#117)
- *(deps)* Update rust crate anyhow to v1.0.99 (#118)
- *(deps)* Update rust crate clap to v4.5.45 (#119)
- *(deps)* Update rust crate serde_json to v1.0.143 (#120)
- *(deps)* Update rust crate regex to v1.11.2 (#122)
- *(deps)* Update rust crate clap to v4.5.46 (#123)
- *(deps)* Update rust crate tracing-subscriber to v0.3.20 [security] (#124)
- *(deps)* Update rust crate clap to v4.5.47 (#125)
- Inherit shell config correctly (#126)

## [0.5.0] - 2025-07-28

### 🚀 Features

- Change default UI (#108)

### 🐛 Bug Fixes

- Show quit key (#109)

## [0.4.3] - 2025-07-27

### 🐛 Bug Fixes

- *(deps)* Update rust crate petgraph to v0.8.2 (#94)
- *(deps)* Update rust crate clap to v4.5.40 (#95)
- *(deps)* Update rust crate libc to v0.2.173 (#96)
- *(deps)* Update rust crate libc to v0.2.174 (#97)
- *(deps)* Update rust crate human-panic to v2.0.3 (#100)
- *(deps)* Update rust crate clap to v4.5.41 (#101)
- *(deps)* Update rust crate serde_json to v1.0.141 (#102)
- *(deps)* Update rust crate axoupdater to v0.9.1 (#103)
- Handle up-to-date as success (#105)
- *(deps)* Update rust crate console to 0.16.0 (#98)
- *(deps)* Update rust crate which to v8 (#93)
- *(deps)* Update rust crate tokio to v1.47.0 (#69)
- *(deps)* Update rust crate indexmap to v2.10.0 (#65)
- Fix up-to-date icon (#107)

## [0.4.2] - 2025-05-28

### 🐛 Bug Fixes

- Fix CUI exit code (#92)

## [0.4.1] - 2025-05-28

### 🐛 Bug Fixes

- *(deps)* Update rust crate clap to v4.5.39 (#90)
- Fix wrong exit code (#91)

## [0.4.0] - 2025-05-24

### 🚀 Features

- Enable to stop/restart task from TUI (#82)
- Add `force` flag (#83)

### 🐛 Bug Fixes

- Handle panic correctly (#84)
- Log fatal errors (#86)
- Stop/restart tasks gracefully (#87)
- Modify shutdown timeout (#88)

### 🚜 Refactor

- Reorganize modules (#79)
- Invert `no_quit` flag (#81)
- Refactor visitor (#80)

## [0.3.3] - 2025-05-13

### 🐛 Bug Fixes

- *(deps)* Update rust crate tokio to v1.44.2 [security] (#68)
- *(deps)* Update rust crate petgraph to 0.8.0 (#67)
- *(deps)* Update rust crate crossterm to 0.29.0 (#66)
- Fix wrong search results of wrapped lines (#71)
- The search results should  begin from the current scrollback (#77)
- *(deps)* Update rust crate petgraph to v0.8.1 (#78)
- *(deps)* Update rust crate clap to v4.5.38 (#73)
- *(deps)* Update rust crate once_cell to v1.21.3 (#76)
- *(deps)* Update rust crate anyhow to v1.0.98 (#72)
- *(deps)* Update rust crate libc to v0.2.172 (#75)
- Workaround release error

### ⚙️ Miscellaneous Tasks

- Update Renovate config
- Run release workflow on ubuntu-latest (#74)
- Update Renovate config

## [0.3.2] - 2025-03-23

### 🐛 Bug Fixes

- Fix includes merging (#63)

## [0.3.1] - 2025-03-22

### 🐛 Bug Fixes

- Exit 1 if any process exits with non-zero code (#61)
- Allow empty command (#62)

## [0.3.0] - 2025-03-22

### 🚀 Features

- Add `var` and `env` option (#59)
- Add `includes` field (#60)

### 🐛 Bug Fixes

- Exit with code 1 on error (#58)
- *(deps)* Update all patch updates (#37)

## [0.2.2] - 2025-03-17

### 🐛 Bug Fixes

- Reduce timeout (#55)
- Start period behavior (#56)
- Exit if all target tasks completed (#57)

## [0.2.1] - 2025-03-14

### 🐛 Bug Fixes

- *(deps)* Update rust crate indexmap to v2.8.0 (#33)
- *(deps)* Update rust crate tokio to v1.44.0 (#44)
- *(deps)* Update rust crate once_cell to v1.21.0 (#38)
- Treat number as string (#53)
- Handle deps with same vars (#54)
- *(deps)* Update rust crate portable-pty to 0.9.0 (#40)

## [0.2.0] - 2025-03-12

### 🚀 Features

- Add self updater (#52)

## [0.1.15] - 2025-03-12

### 🐛 Bug Fixes

- Disable dist updater (#51)

## [0.1.14] - 2025-03-12

### 🐛 Bug Fixes

- Release test

## [0.1.1] - 2025-02-13

<!-- generated by git-cliff -->
