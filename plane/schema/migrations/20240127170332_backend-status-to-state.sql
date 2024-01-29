-- drob backend_status table
drop table if exists backend_status;

-- create backend_state table
create table backend_state (
    id serial primary key,
    backend_id varchar(255) references backend(id) not null,
    state jsonb not null,
    created_at timestamptz not null default now()
);

comment on table backend_state is 'A history of state changes across all backends.';
comment on column backend_state.backend_id is 'The backend the state change refers to.';
comment on column backend_state.state is 'The state of the backend.';
comment on column backend_state.created_at is 'The time the state change was received.';

create index idx_backend_state_created_at on backend_state(backend_id, created_at);

