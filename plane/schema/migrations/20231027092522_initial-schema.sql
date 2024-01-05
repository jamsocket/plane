create table controller (
    id varchar(255) primary key,
    first_seen timestamptz not null default now(),
    last_heartbeat timestamptz not null,
    is_online bool not null,
    plane_version varchar(255) not null,
    plane_hash varchar(255) not null,
    ip inet not null
);

comment on table controller is 'Self-reported information about controllers.';
comment on column controller.first_seen is 'The first time the controller came online.';
comment on column controller.last_heartbeat is 'The last time the controller sent a heartbeat.';
comment on column controller.is_online is 'Whether the controller is online (self-reported; if a controller dies suddenly this will not be updated).';
comment on column controller.plane_version is 'The version of plane running on the controller.';
comment on column controller.plane_hash is 'The git hash of the plane version running on the controller.';
comment on column controller.ip is 'The last-seen IP of the controller (as seen from the Postgres server)';

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

comment on table node is 'Information about nodes (drones, proxies, DNS servers).';
comment on column node.kind is 'A string representing the kind of node this is (serialized types::NodeKind).';
comment on column node.name is 'A string name provided by the node, unique within a cluster.';
comment on column node.cluster is 'The cluster the node belongs to. May be null if the node is cross-cluster (currently only DNS servers may have a null cluster).';
comment on column node.plane_version is 'The version of plane running on the node.';
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

comment on table drone is 'Information about drones used for scheduling.';
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

comment on table backend is 'Information about backends. A row is created when a backend is scheduled.';
comment on column backend.cluster is 'The cluster the backend belongs to.';
comment on column backend.last_status is 'The last status received from the backend.';
comment on column backend.last_status_time is 'The time the last status was received from the backend.';
comment on column backend.cluster_address is 'The address (IP:PORT) of the backend within the cluster.';
comment on column backend.exit_code is 'The exit code of the backend (null if it has not exited; may remain null if exit is forced).';
comment on column backend.drone_id is 'The drone the backend is assigned to.';
comment on column backend.expiration_time is 'A hard deadline after which the controller will initiate a graceful termination of the backend.';
comment on column backend.last_keepalive is 'The last time a proxy sent a keepalive for this backend.';
comment on column backend.allowed_idle_seconds is 'The number of seconds the backend is allowed to be idle (no inbound connections alive) before it is terminated.';

create index idx_backend_drone_id on backend(cluster, drone_id) where last_status != 'Terminated';
comment on index idx_backend_drone_id is 'An index for identifying running backends on a particular drone.';

create table backend_status (
    id serial primary key,
    backend_id varchar(255) references backend(id) not null,
    status varchar(255) not null,
    created_at timestamptz not null default now()
);

comment on table backend_status is 'A history of status changes across all backends.';
comment on column backend_status.backend_id is 'The backend the status change refers to.';
comment on column backend_status.status is 'The status of the backend.';
comment on column backend_status.created_at is 'The time the status change was received.';

create index idx_backend_status_created_at on backend_status(backend_id, created_at);

create table backend_action (
    id varchar(255) primary key,
    backend_id varchar(255) references backend(id),
    drone_id int not null references drone(id),
    action jsonb not null,
    created_at timestamptz not null default now(),
    acked_at timestamptz
);

comment on table backend_action is 'Actions which are either queued to take place, or have taken place, on each backend.';
comment on column backend_action.backend_id is 'The backend the action applies to.';
comment on column backend_action.drone_id is 'The drone associated with the backend_id. This is denormalized from the backend table so that we can efficiently index it.';
comment on column backend_action.action is 'A JSON representation of the action to take. Deserializes to a BackendActionMessage.';
comment on column backend_action.created_at is 'The time the action was created.';  
comment on column backend_action.acked_at is 'The time the action was acked by the drone. Null if the action has not been acked. Will be re-sent on drone reconnect if not already acked.';

create index idx_backend_action_pending on backend_action(drone_id, created_at) where acked_at is null;
create index idx_backend_action_backend on backend_action(backend_id);

create table backend_key (
    id varchar(255) primary key references backend(id) not null,
    namespace varchar(255) not null,
    key_name varchar(255) not null,
    tag varchar(255) not null,
    expires_at timestamptz not null,
    fencing_token bigint not null,
    allow_renew bool not null default true
);

comment on table backend_key is 'Information about the key associated with each backend.';
comment on column backend_key.id is 'The id of the backend the key is associated with.';
comment on column backend_key.namespace is 'The namespace the key belongs to.';
comment on column backend_key.key_name is 'The name of the key, unique within the namespace.';
comment on column backend_key.tag is 'A value which must match for an existing backend to be returned based on key.';
comment on column backend_key.expires_at is 'The time the key expires, unless renewed first.';
comment on column backend_key.fencing_token is 'A number that monotonically increases when the same key is re-acquired, but stays the same across refreshes.';
comment on column backend_key.allow_renew is 'If false, the key cannot be renewed for this backend, forcing the backend to be terminated.';

create unique index idx_namespace_name on backend_key(namespace, key_name);

create table token (
    token varchar(255) primary key,
    backend_id varchar(255) references backend(id) not null,
    expiration_time timestamptz not null,
    username varchar(255),
    auth jsonb not null,
    secret_token varchar(255) not null
);

comment on table token is 'Information about tokens used to make authorized connections to backends.';
comment on column token.token is 'The token used to make the connection.';
comment on column token.backend_id is 'The backend the token is associated with.';
comment on column token.expiration_time is 'The time the token expires.';
comment on column token.username is 'The username associated with the token.';
comment on column token.auth is 'A JSON representation of arbitrary auth metadata which is passed to the backend.';
comment on column token.secret_token is 'A secret token optionally used for secondary authentication.';

create table event (
    id serial primary key,
    kind varchar(255) not null,
    key varchar(255),
    created_at timestamptz not null default now(),
    data jsonb not null
);

comment on table event is 'A history of events that have been broacast in the system.';
comment on column event.kind is 'The kind of event (value of NotificationPayload::kind()).';
comment on column event.key is 'An optional key associated with the event. Along with "kind", acts like a pub/sub "subject". Subscriptions can filter on this key.';
comment on column event.created_at is 'The time the event was created.';
comment on column event.data is 'A JSON representation of the event payload. Must be a type that implements NotificationPayload.';

create index idx_event_created_at on event(created_at);

create table acme_txt_entries (
    cluster varchar(255) primary key,
    leased_at timestamptz not null default now(),
    leased_by int references node(id) not null,
    txt_value varchar(255)
);

comment on table acme_txt_entries is 'TXT entries used for ACME DNS challenges. Doubles as a leasing mechanism to ensure that only one drone can do an ACME challenge at a time.';
comment on column acme_txt_entries.cluster is 'The cluster the TXT entry is associated with.';
comment on column acme_txt_entries.leased_at is 'The time the TXT entry was leased.';
comment on column acme_txt_entries.leased_by is 'The proxy that last leased the entry.';
comment on column acme_txt_entries.txt_value is 'The TXT value of the entry.';
