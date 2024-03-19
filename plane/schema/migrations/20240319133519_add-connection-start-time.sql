alter table node add column "last_connection_start_time" timestamptz;

comment on column node.last_connection_start_time is 'The time of the most recent connection to a controller.';
