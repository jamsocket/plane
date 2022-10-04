# Plane

[![Tests](https://github.com/drifting-in-space/plane/actions/workflows/tests.yml/badge.svg)](https://github.com/drifting-in-space/plane/actions/workflows/unit-tests.yml)

**Plane** allows web applications to create [**session backends**](https://driftingin.space/posts/session-lived-application-backends),
which are server processes dedicated to individual (or small groups of) users.

The server processes can be any server that speaks HTTP, including WebSocket servers. Plane provides
an API for spinning these servers up when a new user connects, and automatically terminates
them when the user disconnects.

Plane is still new and evolving, and the API may change. If you're interested in using it, we'd love
to hear about your use case and help you get started. Feel free to [open an issue on GitHub](https://github.com/drifting-in-space/plane/issues)
or contact [Paul](https://github.com/paulgb) at [paul@driftingin.space](mailto:paul@driftingin.space).

## Open-source Roadmap

We are in the process of extracting code from our platform monorepo into the Plane open source project. The goal is for Plane to provide all the pieces you need to run session-lived backend infrastructure.

- [x] Agent
- [x] Proxy
- [x] Certificate fetcher
- [x] DNS server
- [x] Scheduler
- [ ] Docs

_The original version of Plane was based on Kubernetes. It's available in the [original-kubernetes-based-plane](https://github.com/drifting-in-space/plane/tree/original-kubernetes-based-plane) branch._

## Use cases

Plane is intended for cases where a web app needs a dedicated, stateful back-end to talk to for the
duration of a session. One area where this approach is currently common is web-based IDEs like
[GitHub Codespaces](https://github.com/features/codespaces), which spin up a container for each user
to run code in. It's also useful as a back-end for real-time collaboration, when the document state
is non-trivial and needs more than just a Pub/Sub architecture (see e.g.
[Figma's description](https://www.figma.com/blog/rust-in-production-at-figma/) of how they run one
process per active document.)

By making it low-stakes to experiment with this architecture, our hope is
that Plane will help developers find new use-cases, like loading a large dataset into memory
to allow low-latency interactive data exploration.

Depending on your needs, it may also make sense as a back-end for games and virtual spaces, but also
see [Agones](https://agones.dev/site/) for that use case.

## Overview

Conceptually, Plane provides a way to dynamically spin up an internet-connected, HTTPS-ready Docker container,
called a **backend**.

Each backend gets its own unique hostname, and Plane shuts it down when there are no open connections to
a backend for a (configurable) period of time.

The backends can be distributed across multiple machines, and across different geographic
locations.

## Architecture

![Plane architecture diagram.](https://raw.githubusercontent.com/drifting-in-space/plane/main/architecture.svg)

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
