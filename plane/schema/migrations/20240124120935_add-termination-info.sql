-- This information is part of the state message.
alter table backend drop column exit_code;

-- Add a state column to the backend table.
alter table backend add column "state" jsonb;

comment on column backend."state" is 'The most recent state of the backend.';

-- Set the state to an empty object for all backends.
update backend set "state" = '{}';

-- Make the state column non-nullable.
alter table backend alter column "state" set not null;
