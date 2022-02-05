# Spawner

[![Build](https://github.com/drifting-in-space/spawner/actions/workflows/docker-publish.yml/badge.svg)](https://github.com/drifting-in-space/spawner/actions/workflows/docker-publish.yml)

**Spawner** allows web applications to create **session-lived backends**, which are server processes
dedicated to individual (or small groups of) users.

The server processes can be any server that speaks HTTP, including WebSocket servers. Spawner gives
you an API for spinning these servers up when a new user connects, and automatically terminates
them when the user disconnects.

Spawner isn't production-ready quite yet, but if you're interested in using it in production, we'd love
to hear about your use case and help you try it out. Feel free to open an issue on GitHub or contact
us at [hi@driftingin.space](mailto:hi@driftingin.space).

## Use cases

Spawner is intended for cases where a web app needs a dedicated, stateful back-end to talk to for the
duration of a session. One area where this approach is currently common is web-based IDEs like
[GitHub Codespaces](https://github.com/features/codespaces), which spin up a container for each user
to run code in. It's also useful as a back-end for real-time collaboration, when the document state
is non-trivial and needs more than just a relay server (see e.g.
[Figma's description](https://www.figma.com/blog/rust-in-production-at-figma/) of how they run one
process per active document.)

By making it low-stakes to experiment with this architecture, our hope is
that Spawner will help developers find new use-cases, like loading a large dataset into memory
to allow low-latency interactive data exploration.

Depending on your needs, it may also make sense as a back-end for games and virtual spaces, but also
see [Agones](https://agones.dev/site/) for that use case.

## Architecture

![Spawner architecture diagram](docs/diagram.svg)

## More info & getting started

See the [docs](https://github.com/drifting-in-space/spawner/blob/master/docs/README.md) directory
for more information.
