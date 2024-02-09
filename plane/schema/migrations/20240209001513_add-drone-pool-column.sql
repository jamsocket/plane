-- Add pool column to drone table

alter table drone add column pool text not null default '';

comment on column drone.pool is 'The pool to which the drone is assigned (default pool is an empty string).';

