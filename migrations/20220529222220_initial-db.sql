
-- Describes a Docker resource.
create table "resource" (
    -- The name of this resource within Docker.
    "name" text primary key,

    -- The type of this docker resource, e.g. container, network.
    "kind" text not null,

    -- The backend to which this resource relates.
    "backend" text not null default (unixepoch())
);

-- Describes a route that the proxy should resolve.
create table "route" (
    -- Subdomain to route.
    "backend" text primary key,

    -- IP:port combo of the destination to proxy to.
    "address" text not null,

    -- The timestamp this route last had an active connection.
    -- The proxy may debounce this by a reasonable amount (e.g. 5 seconds)
    -- to avoid excessive writes. If the connection is currently active,
    -- this is NULL.
    "last_active" integer
);
