---
title: Installation
description: Installation methods for Firepit
outline: deep
---

# Installation

## Installer Script

To install the latest version of Firepit, run the following command.

```bash
curl -LsSf https://github.com/kota65535/firepit/releases/latest/download/firepit-installer.sh | sh
```

To install the specific version, replace the `{version}` placeholder with the desired version number.

```bash
curl -LsSf https://github.com/kota65535/firepit/releases/download/{version}/firepit-installer.sh | sh
```

## Mise

[Mise](https://mise.jdx.dev/) is a cross-platform package manager that acts as a "frontend" to a variety
of other package managers "backends" such as `asdf`, `ubi` and `github`.

If using Mise, we recommend using the `github` backend to install Firepit.

```shell [github]
mise use -g github:kota65535/firepit@latest
mise install
```
