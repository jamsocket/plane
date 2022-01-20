# Spawner

[![Spawner Image](https://github.com/drifting-in-space/spawner/actions/workflows/docker-publish-spawner.yml/badge.svg)](https://github.com/drifting-in-space/spawner/actions/workflows/docker-publish-spawner.yml)
[![Sidecar Image](https://github.com/drifting-in-space/spawner/actions/workflows/docker-publish-sidecar.yml/badge.svg)](https://github.com/drifting-in-space/spawner/actions/workflows/docker-publish-sidecar.yml)
[![Sweeper Image](https://github.com/drifting-in-space/spawner/actions/workflows/docker-publish-sweeper.yml/badge.svg)](https://github.com/drifting-in-space/spawner/actions/workflows/docker-publish-sweeper.yml)

**Spawner** is a bridge between a web application and Kubernetes. It allows a web application to
create **session-lived backends**, which are server processes dedicated to individual (or small
groups of) users.

The processes themselves are just regular HTTP servers. Spawner coordinates with
a reverse proxy, so that your client-side code can talk directly to these servers. *Session-lived*
means that the containers are spun up for each user, and spun down once they have no more active
connections.

## Use cases

Spawner is intended for cases where a web app needs a dedicated, stateful back-end to talk to for the
duration of a session. One area where this approach is currently common is web-based IDEs like
[GitHub Codespaces](https://github.com/features/codespaces), which spin up a container for each user
to run code in. It's also useful as a back-end for real-time collaboration, when the document state
is non-trivial and needs more than just a relay server (see e.g.
[Figma's description](https://www.figma.com/blog/rust-in-production-at-figma/) of how they run one
process per active document.)

By making it low-stakes to experiment with this architecture, our hope is
that Spawner will help developers discover new use-cases, like loading a large dataset into memory
to allow low-latency interactive data exploration.

Depending on your needs, it may also make sense as a back-end for games and virtual spaces, but also
see [Agones](https://agones.dev/site/) for that use case.

## How it works

Spawner is built on top of Kubernetes. It provides a `SessionLivedBackend` resource type, and provides
a number of pieces for managing the lifecycle of a session-lived backend.

In order to give users maximimum flexibility in structuring their system, Spawner is made up of a
number of self-contained programs, with minimal and well-defined interfaces.

- `spawner`: a Kubernetes [controller](https://kubernetes.io/docs/concepts/architecture/controller/)
  which watches for the creation of `SessionLivedBackend` resources and sets up `Pod` and `Service`
  resources for each one, and (optionally) injects each `Pod` with the `sidecar` used for monitoring
  connections.
- `sidecar`: a WebSocket-aware HTTP proxy sidecar that also provides an event stream appropriate for
  determining when a server becomes idle, used for terminating services.
- `sweeper`: a service that listens to the event stream of `sidecar` event streams for all the
  `SessionLivedBackend`-controlled `Pod`s on a given node, and terminates them when they become idle.

Additionally, the `cli` codebase contains a command-line helper for tasks like creating a
`SessionLivedBackend` from the command-line. It is just a wrapper on the Kubernetes API, which can
also be used to create `SessionLivedBackend`s with more control.

## Getting started

First, you should have a running Kubernetes cluster and the Kubernetes client, `kubectl`.
The Kubernetes cluster could be a local install of [minikube](https://minikube.sigs.k8s.io/docs/start/),
or a cloud install like [Google Kubernetes Engine](https://cloud.google.com/kubernetes-engine).
(If you are setting up Kubernetes just to try Spawner, I recommend starting with minikube.)

Before continuing, make sure that you can run `kubectl get pods` without error (it's fine if it
returns an empty list.)

A sample cluster configuration is provided in the `cluster` folder of this repo. It uses
[Kustomize](https://kustomize.io/), which is built in to Kubernetes. To install it, run:

    cd cluster
    kubectl apply -k .
    cd ../

This will install the following in your Kubernetes cluster:

- A custom resource definition `SessionLivedBackend`.
- A namespace `spawner`, within which the backends (but not control plane) will run.
- A service account `spawner`, with the ability to manage certain Kubernetes resources.
- A deployment `spawner`, which runs the core Kubernetes controller used to create
  backing resources for `SessionLivedBackend`s.
- A daemon set `sweeper`, which is used to clean up idle backends.
- A deployment `nginx`, which is a reverse proxy that routes HTTP requests to the
  appropriate session-lived backend container.

To create a session-lived backend (in this example, for a multiplayer game), you can
use a provided demo:

    kubectl create -f demo/drop-four.yaml

This will create a `SessionLivedBackend`.

*TODO: describe how to connect to it.*
