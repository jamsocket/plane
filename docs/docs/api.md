---
sidebar_position: 3
---

# API

Communication with the controller occurs through a JSON-over-[NATS](https://nats.io) API.

This API should not yet be considered stable -- it may change between Plane versions and in order to move quickly, we do not yet enforce cross-compatibility between versions. We're [in the process of stabilizing it](https://github.com/drifting-in-space/plane/issues/138), after which it will abide by [semver](https://semver.org/).

## Connecting

To send API requests, the sender must connect to the same NATS cluster that Plane is connected to. NATS provides [client libraries](https://docs.nats.io/running-a-nats-service/clients) for doing this in many different languages.

NATS is a pub/sub message bus, in which messages are published to a “subject”, which is an arbitrary string. Plane interacts with a client by publishing and subscribing to specific subjects.

## Clusters

To make filtering messages easier (and eventually, to facilitate cluster-level permissioning), some subjects include a cluster name. Cluster names are domain names, but the period (`.`) has a special meaning in NATS. To avoid conflating the two, when clusters appear in subjects, periods are replaced with an underscore (`_`).

## Spawning processes

To spawn a process, send a request to the subject `cluster.{cluster_name}.schedule`. For example, if you configured a drone with the cluster `plane.dev`, your subject would be `cluster.plane_dev.schedule`.

The format of the message looks like this:

```javascript
{
    cluster: "plane.dev",   // Name of cluster to spawn on (should match the cluster of the drone you started.)
    max_idle_secs: 30,      // How long a process can have no connections before Plane shuts it down.
    metadata: {},           // Arbitrary key/value pairs to associate with this process, currently used only for logging.
    executable: {           // Specification of the process you want to run.
        image: "ghcr.io/drifting-in-space/demo-image-drop-four", // The OCI/Docker image you want to run.
        env: {},            // Environment variables to pass to the process.
    }
}
```

Some optional fields have been omitted; see the [ScheduleRequest](https://github.com/drifting-in-space/plane/blob/main/core/src/messages/scheduler.rs#L31) type definition for a full list.

If successful, NATS will respond with a message that looks like this:

```javascript
{
    "Scheduled": {
        "drone": "c6486564-699e-46d6-bd08-4bd72e4eb8c0",
        "backend_id": "546a8f81-125a-4930-9b5a-25172100ce78"
    }
}
```

The hostname associated with the new container is `{backend_id}.{cluster}`, so in this case, `546a8f81-125a-4930-9b5a-25172100ce78.plane.dev`. If we had set up DNS on plane.dev to point to the Plane controller,
HTTPS traffic sent to that hostname would be routed to the container we just spawned.

## Status and other messages

Status messages and other message types are not yet documented, but the schema definitions can be found in the [plane/core/src/messages](https://github.com/drifting-in-space/plane/tree/main/core/src/messages) directory for those eager to try them.
