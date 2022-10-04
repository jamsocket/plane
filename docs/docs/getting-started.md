# Getting Started with Spawner

## Introduction

Spawner is a server orchestrator that allows you to spin up many ephemeral instances of containers, and connect directly to those containers over HTTPS.

In contrast to traditional web servers, which are shared between multiple users, these containers can act more like a background process that happens to
run remotely. You can think of Spawner backends as an extension of your client-side app that happens to run on the server.

## Prerequisites

To follow this getting started guide, you'll need the following:

- Docker Engine
- Docker Compose
- Spawner [source code](https://github.com/drifting-in-space/spawner)

Running Spawner requires **Docker Engine**. For Linux, installation instructions for popular distributions are [available here](https://docs.docker.com/engine/).

For Mac and Windows users, the easiest way to get Docker Engine is through [**Docker Desktop**](https://docs.docker.com/desktop/).

This getting started guide uses [**Docker Compose**](https://docs.docker.com/compose/install/linux/) to simplify getting an environment set up.

You'll also need a clone of the Spawner repo to follow along. You can obtain one with git:

```bash
git clone https://github.com/drifting-in-space/spawner
```

This tutorial will use pre-built Spawner container images so that you don't have to build anything locally. If you do want to compile Spawner locally,
you'll also want to [install **Rust**](https://www.rust-lang.org/tools/install).

## Running locally

Use docker-compose to start a local instance of Spawner:

```bash
cd spawner
docker compose up
```

A full production Spawner installation requires configuring DNS records and hosting on a public IP. We didn't want you to have to do all that just to
try it out, so our sample configuration includes an instance of Firefox configured with DNS and certificate settings for testing it out. This instance
of Firefox runs within Docker and is accessible through your regular browser by opening [http://localhost:8080](http://localhost:8080).


