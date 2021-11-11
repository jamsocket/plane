# spawner-sidecar

Spawner Sidecar is a lightweight tool for remotely monitoring whether a given
TCP port is in use. Spawner uses this to ensure that it only shuts down pods
which do not have active connections.

Although it is built for use with Spawner, it does not depend on Spawner and
could be a useful component when building bespoke Kubernetes controllers. Its
interface is a generic and easy to consume read-only HTTP/JSON API.

Spawner Sidecar works by asking the kernel for information about TCP connections
every 10 seconds (configurable). It uses that information to count the number
of open and waiting connections to the “monitored port”, which defaults to 8080.

Simultaneously, it listens on another port (default 7070) for incoming HTTP
connections, and serves its current connection state as a JSON blob like this:

```json
{
    "active_connections": 0,
    "waiting_connections": 0,
    "seconds_since_active": 20,
    "listening": true
}
```

- **`active_connections`** is the number of TCP connections in the `ESTABLISHED` state.
- **`waiting_connections`** is the number of TCP connections in the `TIME_WAIT` state.
- **`seconds_since_active`** is the number of seconds since `waiting_connections` and `active_connections` have both been 0.
- **`listening`** is true if the monitored TCP port is actively being listened on.

## Usage

The process is intended to be used as a Kubernetes sidecar, although it can be run in
any Linux environment. To use it as a sidecar, add it to the container specification
of the pod you'd like to monitor.

The monitoring provided by this module is meant to tell an external process or system
whether a pod can safely be destroyed without interrupting any connections.

## CLI Options

```
USAGE:
    spawner-sidecar [OPTIONS]

OPTIONS:
    -h, --help
            Print help information

        --monitor-port <MONITOR_PORT>
            The TCP port to monitor connection activity on [default: 8080]

        --refresh-rate-seconds <REFRESH_RATE_SECONDS>
            The rate (in seconds) at which to check for activity [default: 10]

        --serve-port <SERVE_PORT>
            The port to open an HTTP server on to serve metrics requests [default: 7070]
```
