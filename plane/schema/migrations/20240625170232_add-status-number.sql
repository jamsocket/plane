alter table backend add column last_status_number integer;

comment on column backend.last_status_number is 'Number associated with a status, used for ordering.';
