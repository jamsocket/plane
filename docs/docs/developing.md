---
sidebar_position: 4
---

# Developing Plane

## Running Tests

Tests consist of unit tests (which are located alongside the source) and integration tests located in `dev/tests`. Both kinds of tests are run if you run `cargo test` in the root directory.

Integration tests use Docker to spin up dependent services, so require a running install of Docker and to be run as a user with access to `/var/run/docker.sock`. They are intended to run on Linux and may not work on other systems.

Integration tests can be slow because they simulate entire workload lifecycles. This is compounded by the fact that the default Rust test runner only runs tests in parallel if they are in the same test file. For faster test runs, we recommend using [cargo-nextest](https://nexte.st/) as follows:

```
cargo install cargo-nextest
cargo nextest run
```