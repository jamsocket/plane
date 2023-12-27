# Keys Design Doc

Keys are strings that can be provided when calling the `connect` API. They are the feature that allows Plane to return an existing backend instead of creating a new one.

Keys are expressed as strings. For a given key, Plane guarantees that at most one backend will be running at a time. This ensures that that backend can be treated as the authoritative server for some room or external resource while it is running.

A **key configuration** (`KeyConfig`) consists of:
- A **name**: a string value that is used to identify the backend. This string gets passed to the backend as an environment variable.
- A **namespace**: an optional string used in combination with the name to create the (namespace, name) pair that must be unique. This allows key names to be reused across namespaces. The namespace is treated as a Plane-level organizational feature and is not passed to the backend. Not providing a namespace is equivalent to passing an empty string.
- A **tag**: an optional string that must match an existing backend for Plane to return that backend. If a connect request shares the name and namespace with an existing key but the tag does **not** match, Plane will return an error instead of returning the backend.

Namespaces and (especially) tags are advanced features; for most users, supplying **only the name** is sufficient.

Tags are useful for sharing a key space between non-interchangable backends. For example, if you have two different programs that read from the same blob store, you can give each a different tag to ensure that a connect request for one does not return the other.

## Key Semantics

Every backend has an associated key. If one is not provided when calling `connect`, Plane will generate one at random. This key is returned as part of the `connect` response.

Plane attempts to guarantee that if a backend is in the `Starting`, `Waiting`, or `Ready` state, the drone running the backend holds the key. It also attempts to guarantee that if a backend is in the `Terminating` state, the key may be expired but is within a termination grace period (N.B. this is unrelated to the idle backend grace period).

If the drone *can't* renew the key, the drone is responsible for terminating the backend on a fixed timeline. The drone will first attempt to shut down the backend gracefully, but will then terminate it with a hard deadline.

The current implementation also happens to ensure that the lock is held during the `Loading` state, but this is not guaranteed to be the case in the future (this is to accomedate potential “pre-warming” behavior in a future release).

## Key acquisition and renewal

Key **acquisition** is triggered by the controller, after which key **renewal** is triggered by the drone.

The controller acquires the key when it is handling a `connect` request.

The reason that the controller handles key acquisition (instead of the drone acquiring the key when a backend is started) is twofold:
1. It eliminates a race condition where two backends attempt to start a backend with the same key at the same time, and one of them fails to start because the key has been acquired in the meantime.
2. The drone would be blocked on an extra round-trip to the controller to acquire the drone before the backend could start, which would slow down the startup process.

## Timing

A design goal of Plane is to be robust to different network configurations and latencies. A consequence of this is that we can’t rely on clocks being synchronized between the controller and the drone.

As a general rule, Plane never uses timestamps in a way that compares the time from one system's clock to another's. This way, it is not sensitive even to wildly incorrect clocks, as long as they progress at the correct speed (within a generous tolerance band).

Instead, when the drone requests a key renewal, it sends its own current system time to the controller. The controller updates the key’s expiration time in the database using the database's time (ignoring the drone-provided time), but the expiration time it sends back to the drone is based on the time provided by the drone.

When first acquiring a key, the drone is not involved until after the key is acquired, so it doesn't have an opportunity to provide a time. Instead, the drone runs a “heartbeat loop” that sends its local time to the controller on a regular interval. The controller then writes this time to the database, and it is used to acquire locks.

## Fencing Tokens

Each key has an associated **fencing token** which is increased each time the key is acquired, but stays the same each time it is renewed. This is not used by the key system itself, but is provided to the backend itself for optional use as a [fencing token](https://martin.kleppmann.com/2016/02/08/how-to-do-distributed-locking.html).

In practice, the token is created from the epoch timestamp (in milliseconds) of the database at the time the lock is acquired. This behavior is not guaranteed, but the token will always be monotonically increasing.

## Preventing renewal

Plane has a mechanism for preventing a key from being renewed. This can be used to force a backend to terminate within a deadline if the drone is lost but we are not sure if it will come back.

If the `allow_renew` column is set to `true` for a backend key, it will not be reneable (any renewal request will be ignored by the controller). Once the key expires, the 

## Expiration Timeline

Each time a key is acquired or renewed, the controller returns an `AcquiredKey`, which contains three timestamps:
- `renew_at`: the time at which the drone should attempt to renew the key
- `soft_terminate_at`: the time at which the drone should gracefully terminate the backend if it has not renewed the key
- `hard_terminate_at`: the time at which the drone should forcefully terminate the backend

The drone is responsible for asking the controller to renew its key before its expiration (a `renew_at` time provided with the key determines when to do so). If the drone does not receive a renewal response before the `soft_terminate_at` time provided by the controller, it will begin the termination process, first by attempting to gracefully terminate the drone and then (after `hard_terminate_at`) by forcefully terminating it.

## Revocation

If, when attempting to start a backend, its key has already expired, Plane will revoke the key by deleting it from the database (`KeysDatabase::remove_key`).
