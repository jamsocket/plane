-- Adds support for static tokens, where a backend has a single token instead of one per connected user.
-- If static_token is NULL, new tokens will be returned for every user; otherwise,
-- the static token will be returned.

alter table backend
    add column static_token varchar(256);

create index idx_backend_static_token on backend(static_token);
