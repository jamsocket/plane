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

- Linux
- Docker
- Plane [source code](https://github.com/drifting-in-space/plane)

Running Plane requires **Docker** on **Linux**. Installation instructions for popular distributions are [available here](https://docs.docker.com/engine/).

You'll also need a clone of the Plane repo to follow along. You can obtain one with git:

```bash
git clone https://github.com/drifting-in-space/plane
```

This tutorial will use pre-built Plane container images so that you don't have to build anything locally. If you do want to compile Plane locally,
you'll also want to [install **Rust**](https://www.rust-lang.org/tools/install).

## Running locally

Use docker-compose to start a local instance of Plane:

```bash
cd plane/sample-config/compose
docker compose pull
docker compose up
```

A full production Plane installation requires configuring DNS records and hosting on a public IP. We didn't want you to have to do all that just to
try it out, so our sample configuration includes an instance of Firefox configured with DNS and certificate settings for testing it out. This instance
of Firefox runs within Docker and is accessible through your regular browser by opening [http://localhost:3000](http://localhost:3000).

### Using the CLI

Although Plane is primarily used via its NATS API, it also includes a small CLI tool that's useful for testing and debugging.

If you have Rust, you can build and install the CLI by running:

```bash
cargo install --path=cli
```

If you don't, or just want a temporary way to try out Plane, you can create a temporary alias that uses a pre-built Docker image of the cli:

```bash
alias plane-cli="docker run --init --network plane plane/plane-cli --nats=nats://nats"
```

Whichever approach you choose, the below commands should work.

### Spawning a backend

Once you have the special instance of Firefox open, the next step is to “spawn” a backend. For this demo, we’ll spawn a simple browser-based game
called _Drop Four_.

To spawn an instance of the game, 

```bash
plane-cli spawn plane.test ghcr.io/drifting-in-space/demo-image-drop-four
```

The first argument is the “cluster”, which refers both to a pool of machines able to run backends (“drones”), and a domain under which these backends will be given hostnames. The drone we are running locally is set up for a cluster called `plane.test` (see `sample-config/drone.toml`). Attempting to spawn on another cluster will return an error stating that no drones are running on that cluster.

If everything worked, you'll get back a response like this:

```
Backend scheduled.
URL: https://4dea4037-3d2d-4e12-95f9-a2864d9ba8dd.plane.test
Drone: b2cdfb71-b2a8-4d89-9dcd-bb7a4cbbddd4
Backend ID: 4dea4037-3d2d-4e12-95f9-a2864d9ba8dd
```

The URL is the URL that the container will be accessible on. Note that plane.test is not a real domain,
so you won't be able to access it in a real browser. Instead, the Drone in our Docker Compose configuration
is configured to use a debug utility called “path routing”. Instead of using the URL provided, you can
construct a URL using the backend ID provided (make sure you use the **backend ID** and not the **drone ID**
-- they are both UUIDs.)

```bash
BACKEND_ID=4dea4037-3d2d-4e12-95f9-a2864d9ba8dd # replace this with the backend ID returned above

# print a URL that you can open in your browser
echo "http://localhost:8080/_plane_backend=$BACKEND_ID/"

# make a curl request to the backend
curl -D - "http://localhost:8080/_plane_backend=$BACKEND_ID/"
```

The Drone is a unique ID associated with the machine which the backend was scheduled to. In this local
test cluster, we are only running one “Drone”, since we are running everything on the same machine.

The backend ID is a unique identifier for the backend we just created.

### Waiting until the backend is ready

Although Plane allocates and returns a hostname immediately, it does not mean that the backend is ready to
receive traffic. To determine when the backend is ready, we can observe the status:

```bash
plane-cli status
```

This will stream status changes from all backends. You can also pass the backend ID (returned in the last
step) as an additional argument to stream only status updates about a specific backend.
