# Spawner

![Spawner logo](spawner.png)

**Spawner** is a bridge between a web application and Kuberenetes. It allows a web application to
create **session-lived** containers that serve WebSocket or HTTP connections. Spawner coordinates with
a reverse proxy, so that your client-side code can talk directly to these servers. *session-lived*
means that when the user closes the connection, the container is cleaned up.

Currently, Spawner only supports [NGINX](https://www.nginx.com/) as a reverse proxy.

**This is still a work-in-progress. It's demo-stage, and not ready for use in production just yet.** If
you are interested in being an early adopter, though, feel free to open an issue or email me at
[hi@driftingin.space](mailto:hi@driftingin.space).

## How it works

### Service Creation

The Spawner process runs in a pod on your cluster and serves an HTTP API. On startup, it is passed an
`--application-image` argument that specifies the full path of the image for your application container
on a container registry.

### Routing

When your web app wants to create a session-lived container, its backend sends a `POST` request to
`http://hostname-of-spawner:8080/init`. Spawner then asks Kubernetes to create a pod and service for that
session, and returns a `JSON` object containing a URL specific to that session-lived container, like
`https://my-domain.com/p/JE3M/`. This URL can then be passed on to the client-side container, which can
connect to it as a regular HTTP host. The proxy server is configured to map paths under the root,
so that `https://my-domain.com/p/JE3M/my-file.txt` is internally routed to
`http://hostname-of-pod/my-file.txt`.

### Service Destruction

When Spawner detects that a container has not served a request for some (configurable) interval, it
will shut down the pod and delete the service. It can determine whether a pod has served a request
in one of two ways:

1. The pod can serve a `/status` endpoint which returns a `JSON` blob that looks like this:

```json
{
  "active_connections": 2,
  "seconds_inactive": 0,
  "listening": true,
}
```

- `active_connections` is the number of active connections (e.g. WebSocket connections) to the server.
- `seconds_inactive` is the amount of time elapsed since the last connection.
- `listening` is true if the server is currently accepting new connections.

At least one of `active_connections` or `seconds_inactive` should be zero. Currently, only
`seconds_inactive` is used; the container is shut down when it passes a threshold value. Eventually,
the other values may be exposed through a monitoring interface.

2. The [sidecar](sidecar) process can be injected into your pod. The sidecar process shares a network
namespace with the application container, so it can ask the OS for active TCP connections on the
application container's port. It uses this information to serve the same `/status` interface, but
on a different port.
