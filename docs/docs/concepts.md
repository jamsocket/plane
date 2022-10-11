---
sidebar_position: 2
---

# Concepts

## Session Backends

Plane is an orchestrator for **session backends**. You can think of session backends as ephemeral
processes (specifically, instances of a Docker container).

The term “backend” refers to the fact that you can connect to these containers from a frontend, i.e.
client code running in the browser. The term “session” refers to the fact that they are bound to the
lifecycle of a user session, that is, when all inbound connections from the frontend(s) are dropped,
the backend is terminated.

## Clusters

Clusters refer to pools of servers that are capable of running backends. Each cluster is uniquely
referred to by a hostname (e.g. `plane.dev`). Every backend that belongs to a cluster is given
a unique subdomain under that cluster’s hostname (e.g. `302d996a-6b3a-433b-a1ad-c0ad47ba1575.plane.dev`).

Plane is capable of supporting multiple clusters at once, but servers that run backends (**drones**,
see below) are only capable of belonging to one cluster at a time.

## Controller and Drone

Functionally, Plane is divided into two parts: the **controller**, and the **drone**.

The drones are the workers that run the actual backends. The Plane drone codebase runs on each
drone machine and is responsible for coordinating with a local Docker instance to run backends, as
well as dispatching inbound traffic to the appropriate container.

The controller is the dispatch center, responsible for accepting external requests for backends and
deciding which drone to run them on. The controller is also responsible for routing traffic to the
appropriate drone by serving DNS, and vouching for drones when they need to request HTTPS certificates.
