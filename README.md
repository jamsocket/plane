<div style="postion: relative; width: 337px; height: 110px;">
    <a href="https://plane.dev#gh-light-mode-only" style="position: absolute;">
        <img src="./resources/plane-logo-light.svg" alt="Plane logo" />
    </a>
    <a href="https://plane.dev#gh-dark-mode-only" style="position: absolute;">
        <img src="./resources/plane-logo-dark.svg" alt="Plane logo" />
    </a>
</div>

[![Docker image](https://img.shields.io/docker/v/plane/plane)](https://hub.docker.com/r/plane/plane/tags)
[![Build Docker Image](https://github.com/jamsocket/plane/actions/workflows/build-image.yml/badge.svg)](https://github.com/jamsocket/plane/actions/workflows/build-image.yml)
[![Tests](https://github.com/jamsocket/plane/actions/workflows/tests.yml/badge.svg)](https://github.com/jamsocket/plane/actions/workflows/tests.yml)
[![Chat on Discord](https://img.shields.io/static/v1?label=chat&message=discord&color=404eed)](https://discord.gg/N5sEpsuhh9)
[![crates.io](https://img.shields.io/crates/v/plane.svg)](https://crates.io/crates/plane)

Plane is a distributed system for **running stateful WebSocket backends at scale**. Plane is heavily inspired by [Figma’s mulitplayer infrastructure](https://www.figma.com/blog/rust-in-production-at-figma/), which dynamically spawns a process for each active document.

Use cases include:
- Scaling up [authoritative multiplayer backends](https://driftingin.space/posts/you-might-not-need-a-crdt).
- Running isolated code environments (like REPLs, code notebooks, and LLM agent sandboxes).
- Data-intensive applications that need a dedicated high-RAM process for each active user session.

## How Plane works

You can think of Plane as a distributed hashmap, but instead of storing data, it stores running processes. When you ask Plane for the process associated with a key (via an HTTP API), it either returns a URL to an existing process, or starts a new process and returns a URL to that.

Plane will keep the process running for as long as there is an open connection (usually a WebSocket connection) to it. Once all connections to a process have been closed for some time threshold, Plane will shut down the process.

Plane guarantees that only one process will be running for each key at any given time, allowing that process to act as an authoritative source of document state for as long as it is running.

### Architecture

Read more about [Plane’s architecture](https://plane.dev/concepts/architecture).

[![Architecture diagram of Plane](./docs/public/arch-diagram.svg)](https://plane.dev/concepts/architecture)

## Learn more

- Read the [quickstart guide](https://plane.dev/quickstart-guide)
- Learn about [Plane concepts](https://plane.dev/concepts/session-backends)
- See instructions for [building and developing Plane locally](https://plane.dev/developing)
- Read about [production deployment](https://plane.dev/deploy-to-prod)
