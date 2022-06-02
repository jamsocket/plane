# Spawner

**Spawner** allows web applications to create [**session-lived backends**](https://driftingin.space/posts/session-lived-application-backends),
which are server processes dedicated to individual (or small groups of) users.

*Spawner is undergoing rapid development and the API may change. The original version of Spawner was based on Kubernetes. It's available in the [original-kubernetes-based-spawner](https://github.com/drifting-in-space/spawner/tree/original-kubernetes-based-spawner) branch.*

The server processes can be any server that speaks HTTP, including WebSocket servers. Spawner provides
an API for spinning these servers up when a new user connects, and automatically terminates
them when the user disconnects.

Spawner is still new and evolving. If you're interested in using it, we'd love
to hear about your use case and help you get started. Feel free to [open an issue on GitHub](https://github.com/drifting-in-space/spawner/issues)
or contact [Paul](https://github.com/paulgb) at [paul@driftingin.space](mailto:paul@driftingin.space).

## Use cases

Spawner is intended for cases where a web app needs a dedicated, stateful back-end to talk to for the
duration of a session. One area where this approach is currently common is web-based IDEs like
[GitHub Codespaces](https://github.com/features/codespaces), which spin up a container for each user
to run code in. It's also useful as a back-end for real-time collaboration, when the document state
is non-trivial and needs more than just a Pub/Sub architecture (see e.g.
[Figma's description](https://www.figma.com/blog/rust-in-production-at-figma/) of how they run one
process per active document.)

By making it low-stakes to experiment with this architecture, our hope is
that Spawner will help developers find new use-cases, like loading a large dataset into memory
to allow low-latency interactive data exploration.

Depending on your needs, it may also make sense as a back-end for games and virtual spaces, but also
see [Agones](https://agones.dev/site/) for that use case.

## Overview

Conceptually, Spawner provides a way to dynamically spin up an internet-connected, HTTPS-ready Docker container,
called a **backend**.

Each backend gets its own unique hostname, and Spawner shuts it down when there are no open connections to
a backend for a (configurable) period of time.

The backends can be distributed across multiple machines, and across different geographic
locations.

## Architecture

Spawner consists of two main pieces, the **controller** and the **drone**, which communicate with each other
over [NATS](https://nats.io/).

The controller consists of:
- The **DNS server**, used to manage routing backend-bound traffic to the right drone, as well as to handle
  [DNS-01 ACME challenges](https://letsencrypt.org/docs/challenge-types/#dns-01-challenge).
- The **scheduler**, which listens for backend requests and picks an available drone to run them on.

The drone consists of:
- An **agent**, which receives instructions from the scheduler to run backends and coordinates with the
  Docker daemon to run them.
- A **proxy**, which routes incoming HTTPS traffic to the appropriate backend.
- A **certificate refresher**, which on launch (and then periodically) refreshes the proxy's HTTPS certificate
  to ensure its continued validity.

## Running Tests

The spawner repository contains two types of tests: unit tests, which are embedded in the Rust codebase, and
interface-level tests written in TypeScript.

To run the Rust tests, run `cargo test` in the root directory.

The TypeScript tests require nodejs and a running Docker daemon. To run these tests, change into the `tests`
directory and run `npm install` to install the dependencies, followed by `npm test` to run the tests.
