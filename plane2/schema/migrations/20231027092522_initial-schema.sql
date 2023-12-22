create table controller (
    id varchar(255) primary key,
    first_seen timestamptz not null default now(),
    last_heartbeat timestamptz not null,
    is_online bool not null,
    plane_version varchar(255) not null,
    plane_hash varchar(255) not null,
    ip inet not null
);

create table node (
    id serial primary key,
    kind varchar(255) not null,
    name varchar(255) not null,
    cluster varchar(255),
    plane_version varchar(255) not null,
    plane_hash varchar(255) not null,
    controller varchar(255) references controller(id),
    ip inet not null
);

create unique index idx_cluster_name on node(cluster, name);

comment on column node.name is 'A string name provided by the node, unique within a cluster.';
comment on column node.kind is 'A string representing the kind of node this is (serialized types::NodeKind).';
comment on column node.cluster is 'The cluster the node is registered with.';
comment on column node.plane_version is 'The version of the plane running on the node.';
comment on column node.plane_hash is 'The git hash of the plane version running on the node.';
comment on column node.controller is 'The controller the node is registered with (null if the node is offline).';
comment on column node.ip is 'The last-seen IP of the node relative to the controller. This is just for reference; drones self-report their IP for use by proxies.';

create table drone (
    id serial primary key references node(id),
    ready boolean not null,
    draining boolean not null default false,
    last_heartbeat timestamptz,
    last_local_time timestamptz
);

comment on column drone.id is 'The unique id of the drone (shared with the node associated with this drone).';
comment on column drone.ready is 'Whether the drone is ready to accept backends.';
comment on column drone.draining is 'Whether the drone is draining. If true, this drone will not be considered by the scheduler.';
comment on column drone.last_heartbeat is 'The last time local_epoch_millis was received from the drone.';
comment on column drone.last_local_time is 'The last reported local timestamp on the drone, used to assign initial key leases when spawning.';

create table backend (
    id varchar(255) primary key,
    cluster varchar(255) not null,
    last_status varchar(255) not null,
    last_status_time timestamptz not null,
    cluster_address varchar(255),
    exit_code int,
    drone_id int references drone(id) not null,
    expiration_time timestamptz,
    last_keepalive timestamptz not null,
    allowed_idle_seconds int
);

comment on column backend.id is 'The unique id of the backend.';
comment on column backend.cluster is 'The cluster the backend belongs to.';
comment on column backend.last_status is 'The last status received from the backend.';
comment on column backend.last_status_time is 'The time the last status was received from the backend.';
comment on column backend.cluster_address is 'The address (IP:PORT) of the backend within the cluster.';
comment on column backend.drone_id is 'The drone the backend is running on.';
comment on column backend.expiration_time is 'A hard deadline after which the controller will initiate a graceful termination of the backend.';
comment on column backend.last_keepalive is 'The last time a proxy sent a keepalive for this backend.';
comment on column backend.allowed_idle_seconds is 'The number of seconds the backend is allowed to be idle (no inbound connections alive) before it is terminated.';

create index idx_backend_drone_id on backend(cluster, drone_id) where last_status != 'Terminated';
comment on index idx_backend_drone_id is 'An index for running/starting backends on a particular drone.';

create table backend_status (
    id serial primary key,
    backend_id varchar(255) references backend(id) not null,
    status varchar(255) not null,
    created_at timestamptz not null default now()
);

create index idx_backend_status_created_at on backend_status(backend_id, created_at);

create table backend_action (
    id varchar(255) primary key,
    drone_id int not null references drone(id),
    backend_id varchar(255) references backend(id),
    action jsonb not null,
    created_at timestamptz not null default now(),
    acked_at timestamptz
);

create index idx_backend_action_pending on backend_action(drone_id) where acked_at is null;
create index idx_backend_action_backend on backend_action(backend_id);

create table backend_key (
    id serial primary key,
    cluster varchar(255) not null,
    namespace varchar(255) not null,
    key_name varchar(255) not null,
    tag varchar(255) not null,
    expires_at timestamptz not null,
    backend_id varchar(255) references backend(id) not null
);

create unique index idx_cluster_namespace_name on backend_key(cluster, namespace, key_name);

create table token (
    token varchar(255) primary key,
    backend_id varchar(255) references backend(id) not null,
    expiration_time timestamptz not null,
    username varchar(255),
    auth jsonb not null,
    secret_token varchar(255) not null
);

create table event (
    id serial primary key,
    kind varchar(255) not null,
    key varchar(255),
    created_at timestamptz not null default now(),
    data jsonb not null
);

create index idx_event_created_at on event(created_at);

create table acme_txt_entries (
    cluster varchar(255) primary key,
    leased_at timestamptz not null default now(),
    leased_by int references node(id) not null, -- refers to the proxy that last leased the entry
    txt_value varchar(255)
)
