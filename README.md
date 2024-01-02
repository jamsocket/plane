**Note:** This branch is an in-progress rewrite of Plane! Please poke around, but don’t use it in production yet, and don’t be surprised if documentation is missing or inconsistent.

<div style="postion: relative; width: 337px; height: 110px;">
    <a href="https://plane.dev" style="position: absolute;">
        <svg width="337" height="110" viewBox="0 0 337 110" fill="none" xmlns="http://www.w3.org/2000/svg">
            <path d="M122.831 3.675H89.8611V73.5H100.781V45.99H122.831C138.371 45.99 144.566 36.12 144.566 24.78C144.566 13.65 138.371 3.675 122.831 3.675ZM100.781 37.275V12.39H120.206C130.076 12.39 133.226 17.64 133.226 24.78C133.226 31.92 130.076 37.275 120.206 37.275H100.781ZM163.464 73.5V0H152.964V73.5H163.464ZM189.868 74.865C200.998 74.865 206.773 68.25 208.873 61.635C208.873 65.835 209.083 69.405 209.503 73.5H219.058C218.638 68.565 218.533 65.415 218.533 61.53V35.49C218.533 25.83 211.393 19.215 196.903 19.215C181.993 19.215 173.698 26.25 173.593 37.38H184.093C184.618 30.765 188.713 27.3 197.323 27.3C206.353 27.3 208.873 30.555 208.873 35.28C208.873 38.955 206.563 40.53 201.523 41.37L188.398 43.575C176.638 45.57 172.438 51.03 172.438 59.43C172.438 69.09 179.158 74.865 189.868 74.865ZM192.073 66.78C186.613 66.78 182.938 64.05 182.938 59.01C182.938 54.18 185.773 51.66 192.283 50.19L199.528 48.51C202.678 47.775 206.038 46.935 208.873 44.835V47.985C208.873 58.485 203.098 66.78 192.073 66.78ZM258.405 19.32C248.325 19.32 242.55 24.99 239.4 32.76V21H228.9V73.5H239.4V51.345C239.4 34.65 245.07 28.035 255.36 28.035C261.975 28.035 267.015 30.66 267.015 41.685V73.5H277.515V40.74C277.515 26.985 272.37 19.32 258.405 19.32ZM311.121 67.41C302.091 67.41 295.371 63.42 294.531 50.19H335.901C336.111 48.825 336.111 47.565 336.111 46.2C336.111 31.185 327.501 19.32 310.806 19.32C292.851 19.32 284.241 31.815 284.241 47.145C284.241 62.685 292.116 75.18 310.806 75.18C329.286 75.18 335.061 64.89 336.006 57.645H326.346C324.876 63.105 320.676 67.41 311.121 67.41ZM310.911 27.09C320.256 27.09 325.506 31.92 326.031 42.525H294.636C295.896 30.87 301.881 27.09 310.911 27.09Z" fill="currentColor"/>
            <path d="M6 4H4C1.79086 4 0 5.79086 0 8V10C0 12.2091 1.79086 14 4 14H6C8.20914 14 10 12.2091 10 10V8C10 5.79086 8.20914 4 6 4Z" fill="currentColor"/>
            <path d="M31 4H19C16.7909 4 15 5.79086 15 8V10C15 12.2091 16.7909 14 19 14H31C33.2092 14 35 12.2091 35 10V8C35 5.79086 33.2092 4 31 4Z" fill="currentColor"/>
            <path d="M66 4H44C41.7909 4 40 5.79086 40 8V10C40 12.2091 41.7909 14 44 14H66C68.2092 14 70 12.2091 70 10V8C70 5.79086 68.2092 4 66 4Z" fill="currentColor"/>
            <path d="M6 19H4C1.79086 19 0 20.7909 0 23V35C0 37.2091 1.79086 39 4 39H6C8.20914 39 10 37.2091 10 35V23C10 20.7909 8.20914 19 6 19Z" fill="currentColor"/>
            <path d="M31 19H19C16.7909 19 15 20.7909 15 23V35C15 37.2091 16.7909 39 19 39H31C33.2092 39 35 37.2091 35 35V23C35 20.7909 33.2092 19 31 19Z" fill="currentColor"/>
            <path d="M66 19H44C41.7909 19 40 20.7909 40 23V35C40 37.2091 41.7909 39 44 39H66C68.2092 39 70 37.2091 70 35V23C70 20.7909 68.2092 19 66 19Z" fill="currentColor"/>
            <path d="M6 44H4C1.79086 44 0 45.7909 0 48V70C0 72.2091 1.79086 74 4 74H6C8.20914 74 10 72.2091 10 70V48C10 45.7909 8.20914 44 6 44Z" fill="currentColor"/>
            <path d="M31 44H19C16.7909 44 15 45.7909 15 48V70C15 72.2091 16.7909 74 19 74H31C33.2092 74 35 72.2091 35 70V48C35 45.7909 33.2092 44 31 44Z" fill="currentColor"/>
            <path d="M66 44H44C41.7909 44 40 45.7909 40 48V70C40 72.2091 41.7909 74 44 74H66C68.2092 74 70 72.2091 70 70V48C70 45.7909 68.2092 44 66 44Z" fill="currentColor"/>
        </svg>
    </a>
</div>

[![Docker image](https://img.shields.io/docker/v/plane/plane-preview)](https://hub.docker.com/r/plane/plane-preview/tags)
[![Build Docker Image](https://github.com/drifting-in-space/plane/actions/workflows/plane2-build-image.yml/badge.svg)](https://github.com/drifting-in-space/plane/actions/workflows/plane2-build-image.yml)
[![Tests](https://github.com/drifting-in-space/plane/actions/workflows/plane2-tests.yml/badge.svg)](https://github.com/drifting-in-space/plane/actions/workflows/plane2-tests.yml)

Plane is infrastructure software for **running stateful WebSocket backends at scale**. Plane is heavily inspired by [Figma’s mulitplayer infrastructure](https://www.figma.com/blog/rust-in-production-at-figma/), which dynamically spawns a process for each active document.

Use cases include:
- Scaling up [authoritative multiplayer backends](https://driftingin.space/posts/you-might-not-need-a-crdt).
- Running isolated code environments (like REPLs, code notebooks, and LLM agent sandboxes).
- Data-intensive applications that need a dedicated high-RAM process for each active user session.

## How Plane works

You can think of Plane as a distributed hashmap, but instead of storing data, it stores running processes. When you ask Plane for the process associated with a key (via an HTTP API), it either returns a URL to an existing process, or starts a new process and returns a URL to that.

Plane will keep the process running for as long as there is an open connection (usually a WebSocket connection) to it. Once all connections to a process have been closed for some time threshold, Plane will shut down the process.

Plane guarantees that only one process will be running for each key at any given time, allowing that process to act as an authoritative source of document state for as long as it is running.

## Example

Imagine a multiplayer document editor. When Sam opens a document with ID `abc123`, the application requests a process from Plane with that key. In this case no process is running, so Plane starts a new one.

When Jane opens the *same* document, the application requests a process from Plane with the same key (`abc123`). This time Plane already has a process running for that key, so it returns a URL which maps to that process.

As long as either Jane or Sam has document `abc123` open, Plane will keep the process associated with that document running. After **both** Jane and Sam have closed the document, Plane will shut down the process.

If Carl later opens the same document, Plane will start a _new_ process for him, possibly on a different machine.

## Quick Start

This repo includes a Docker Compose file that is suitable for running a local development instance of Plane.

This works on Linux and Mac systems with Docker installed.

In the root of this repo, run:

```bash
docker compose -f docker/docker-compose.yml up
```

This tells Docker to run a Postgres database, as well as a minimal Plane stack: one controller, one drone, and one proxy (see below for an explanation).

The Plane Controller will be available at http://localhost:8080, and the Plane Proxy will be available at http://localhost:9090.

### Connecting to a process

The `dev/cli.sh` script runs the Plane CLI, configured to connect to the local Plane Controller.

```bash
dev/cli.sh connect \
    --cluster 'localhost:9090' \
    --image ghcr.io/drifting-in-space/demo-image-drop-four
```

## Running tests

Tests can be run with `cargo test`, but it can be slow because it does not run tests in parallel and some of the tests are slow.

You can use `nextest` to run tests in parallel:

```bash
cargo install nextest
cargo nextest run -j 5
```

The `-j 5` flag tells `nextest` to run 5 tests in parallel. If you set it too high, you may encounter Docker issues.
