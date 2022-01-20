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
