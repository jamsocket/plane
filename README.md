<img src="resources/plane-logo-blue.svg" style="width: 100%" />

[![Tests](https://github.com/drifting-in-space/plane/actions/workflows/tests.yml/badge.svg)](https://github.com/drifting-in-space/plane/actions/workflows/tests.yml)
[![Chat on Discord](https://img.shields.io/static/v1?label=chat&message=discord&color=404eed)](https://discord.gg/N5sEpsuhh9)

Plane is a **container orchestrator for ambitious browser-based applications**. Plane implements an architecture we call [session backends](https://driftingin.space/posts/session-lived-application-backends).

**tl;dr:** Plane lets you spin up instances of any HTTP-speaking container via an API. Plane assigns a unique subdomain to each instance, through which it proxies HTTPS/WebSocket connections. When all inbound connections to a container are dropped, Plane shuts it down. Plane is MIT-licensed and written in Rust.

https://user-images.githubusercontent.com/46173/194342417-b536322c-296c-4e3e-a396-68956db364fe.mov

## Documentation

- [Docs Home](https://plane.dev/)
  - [Getting started guide](https://plane.dev/docs/getting-started)
  - [Concepts](https://plane.dev/docs/concepts)
  - [Deploying Plane](https://plane.dev/docs/deploying)

## Architecture

![Plane architecture diagram.](resources/architecture.svg)

Plane consists of two main pieces, the **controller** and the **drone**, which communicate with each other
over [NATS](https://nats.io/).

The controller consists of:

- The **DNS server**, used to manage routing backend-bound traffic to the right drone, as well as to handle
  [ACME challenges](https://letsencrypt.org/docs/challenge-types/#dns-01-challenge).
- The **scheduler**, which listens for backend requests and picks an available drone to run them on.

The drone consists of:

- An **agent**, which receives instructions from the scheduler to run backends and coordinates with the
  Docker daemon to run them.
- A **proxy**, which routes incoming HTTPS traffic to the appropriate backend.
- A **certificate refresher**, which on launch (and then periodically) refreshes the proxy's HTTPS certificate
  to ensure its continued validity.

## Running Tests

Tests consist of unit tests (which are located alongside the source) and integration tests located in [`dev/tests`](dev/tests). Both kinds of tests are run if you run `cargo test` in the root directory.

Integration tests use Docker to spin up dependent services, so require a running install of Docker and to be run as a user with access to `/var/run/docker.sock`. They are intended to run on Linux and may not work on other systems.

Integration tests can be slow because they simulate entire workload lifecycles. This is compounded by the fact that the default Rust test runner only runs tests in parallel if they are in the same test file. For faster test runs, we recommend using [cargo-nextest](https://nexte.st/) as follows:

```
cargo install cargo-nextest
cargo nextest run
```

## Developing locally

To develop locally, use the `./dev-*.sh` scripts in the root of this repo, in this order:

- Start NATS with `./dev-nats.sh`
- Start the controller with `./dev-controller.sh`
- Start the drone with `./dev-drone.sh`

Then, you can use the CLI with `./dev-cli.sh`. For example, you can spawn a hello-world image like this:

    ./dev-cli.sh spawn plane.test ghcr.io/drifting-in-space/demo-image-hello-world:sha-2ccc18e

And then access it with:

    ./dev-curl.sh [URL returned after spawn]

## Docker containers

**Note that the GitHub-hosted Docker containers (hosted from `ghcr.io`) are for historic use only.** Packages have moved to the [`plane` organization](https://hub.docker.com/u/plane) on Docker Hub.


## Spawning over HTTP

The Plane controller can also optionally start an HTTP server that supports a subset of the NATS API for spawning.

This is intended for development; in production we recommend using the NATS API.

To enable the HTTP server, add an `[http]` block to the controller config. The block requires a `cluster` argument, which specifies the cluster that spawn requests should be spawned to. It also requires a `services` table, which defines a mapping from “service” names to the image to spawn for that service.

```toml
[http]
cluster = "plane.test"
services.hello-world = "ghcr.io/drifting-in-space/demo-image-hello-world:sha-3b58532"
```

By default, the controller HTTP server runs on port 9090. You can send a spawn request over HTTP like this:

```bash
curl \
  -X POST \
  -H "Content-Type: application/json" \
  -d '{}' \
  -D - \
  localhost:9090/service/hello-world/spawn
```
