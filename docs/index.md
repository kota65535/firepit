---
# https://vitepress.dev/reference/default-theme-home-page
layout: home

hero:
  name: Firepit
  text: The Task Runner for Your Terminal
  tagline: |
    A simple, powerful task runner with service management and comfy terminal UI
  image:
    src: logo.png
  actions:
    - theme: brand
      text: Install
      link: /installation
    - theme: alt
      text: Get Started
      link: /getting-started
    - theme: alt
      text: Configuration
      link: /configuration

features:
  - title: Individual task log views
    details: Task logs can be viewed and searched separately, allowing you to focus on what matters
    icon: 📺
  - title: Interaction with running task
    details: Enter the shell of individual tasks and pass inputs to your scripts via stdin
    icon: 🎮
  - title: Service management
    details: Handles long-running tasks with readiness checks and restart policies
    icon: 🚀
  - title: Works with single packages and multilingual monorepos
    details: Wide range of use cases due to its language-agnostic nature and flexible project structure
    icon: 🌱
---
