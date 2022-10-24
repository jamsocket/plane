-- Primary table for backends.
-- Source of truth for lifecycle status.
create table "backend" (
    -- A unique (string) name of this resource.
    "name" text primary key not null,

    -- The spec for this backend, as JSON.
    "spec" text not null,

    -- The current state of this backend, as a string.
    "state" text not null,

    -- The exit code, once the node has exited.
    "exit_code" integer
);

-- Describes a route that the proxy should resolve.
create table "route" (
    "id" integer primary key,

    -- Backend this route is associated with.
    "backend" text,

    -- Subdomain to route.
    "subdomain" text unique not null,

    -- IP:port combo of the destination to proxy to.
    "address" text unique not null,

    -- The timestamp this route last had an active connection.
    -- The proxy may debounce this by a reasonable amount (e.g. 5 seconds)
    -- to avoid excessive writes.
    "last_active" integer not null,

    foreign key("backend") references backend("name")
);
