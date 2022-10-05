---
sidebar_position: 1
---

# Getting Started with Plane

## Introduction

Plane is a server orchestrator that allows you to spin up many ephemeral instances of containers, and connect directly to those containers over HTTPS.

In contrast to traditional web servers, which are shared between multiple users, these containers can act more like a background process that happens to
run remotely. You can think of Plane backends as an extension of your client-side app that happens to run on the server.

## Prerequisites

To follow this getting started guide, you'll need the following:

- Docker
- Plane [source code](https://github.com/drifting-in-space/plane)

Running Plane requires **Docker**. For Linux, installation instructions for popular distributions are [available here](https://docs.docker.com/engine/).

For Mac and Windows users, the easiest way to get Docker is through [**Docker Desktop**](https://docs.docker.com/desktop/).

You'll also need a clone of the Plane repo to follow along. You can obtain one with git:

```bash
git clone https://github.com/drifting-in-space/plane
```

This tutorial will use pre-built Plane container images so that you don't have to build anything locally. If you do want to compile Plane locally,
you'll also want to [install **Rust**](https://www.rust-lang.org/tools/install).

## Running locally

Use docker-compose to start a local instance of Plane:

```bash
cd plane
docker compose up
```

A full production Plane installation requires configuring DNS records and hosting on a public IP. We didn't want you to have to do all that just to
try it out, so our sample configuration includes an instance of Firefox configured with DNS and certificate settings for testing it out. This instance
of Firefox runs within Docker and is accessible through your regular browser by opening [http://localhost:3000](http://localhost:3000).

### Spawning a backend

Once you have the special instance of Firefox open, the next step is to “spawn” a backend. For this demo, we’ll spawn a simple browser-based game
called _Drop Four_.

To spawn an instance of the game, 

```bash
docker run --network plane ghcr.io/drifting-in-space/plane-cli --nats=nats://nats spawn ghcr.io/drifting-in-space/demo-image-drop-four
```

Here's a breakdown of what the command does:

```bash
docker run\                                     # We're running a docker command
--network plane\                                # ...in the network we created by "docker compose up"
ghcr.io/drifting-in-space/plane-cli\            # ...to run the Plane CLI
--nats=nats://nats\                             # ...pointed to the NATS server we started earlier
spawn\                                          # ...and running the "spawn" subcommand
plane.test\                                     # ...on the "plane.test" cluster
ghcr.io/drifting-in-space/demo-image-drop-four  # ...with the argument of the container we want to spawn.
```

### Inspecting the DNS records

Once you’ve spawned a backend, Plane’s DNS server will serve a route for it. To list DNS records being served by plane, run:

```bash
docker run --network plane ghcr.io/drifting-in-space/plane-cli --nats=nats://nats list-dns
```

The result should be 1 DNS record, corresponding to the backend you spawned (if there isn’t, run `docker logs plane-controller` and
`docker logs plane-drone` to see the logs of the controller and drone, respectively. It may just be taking a while to pull the image
the first time.)

### Opening the app

The `list-dns` command above should return a 
