---
title: Getting Started
description: Guide for getting started with Firepit
outline: deep
---

# Getting Started

## Run the First Task

Create a `firepit.yml`.

```yaml
tasks:
  # Task name
  hello:
    # The command to execute in shell
    command: npx -y cowsay "hello,firepit"
```

Run `firepit` in the terminal. You can use the alias `fire`.

```bash
fire hello
```

<img src="/hello.png" alt="hello">

Press `q` key to quit the TUI.

```
% #hello (Finished - Success, Restart: 0/0, Reload: 0, Elapsed: 1s)
 _______________
< hello,firepit >
 ---------------
        \   ^__^
         \  (oo)\_______
            (__)\       )\/\
                ||----w |
                ||     ||
```

The task logs are preserved, and you can view them again even after exiting the TUI.
