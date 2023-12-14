
- in order to avoid clock skew, all times come from the database's clock.

- "name" is a cluster-namespaced, externally generated identifier for a resource (like a drone or proxy instance).
  "id" is reserved for globally-unique canonical identifiers, which may be unique strings or auto-incremented numbers.


- drones should be as close to a relay for docker commands as possible, with the exceptions that:
  - port management is done on the drone side
  - the executable config is a simplified version of the Docker config, because we do not want the controller to have full root access to the host
  - readiness checks happen on the drone side

- in a client/server connection, only the server is aware of the connection. The client treats all outbound messages as ephemeral.
  if the server wants to request initial connection state from the client, it can send a message to the client, which will respond with the requested state.

Locks have a very simple contract:
- The drone requests a lock for a specific backend. Renewals work the same way as the original request.
- Every lock request includes a unique ID. When a lock request is made, the drone will store its current system time in a map associated with that unique ID.
- The drone must hold a lock any time the backend is running. If the drone can't renew a lock, it must shut down the backend.
- The controller receives the lock request.
- The controller checks if the lock is available (meaning it either already belongs to that backend, or it was available). If it is, it sends a lock grant to the drone.
- The controller returns a lock grant to the drone, including the ID of the lock request.
- The drone looks up the lock request in its map, and uses the stored clock time to determine the lock's expiration time on the local system clock.

This approach ensures that even if the controller's clock is skewed, the drone will still be able to determine when the lock expires.

