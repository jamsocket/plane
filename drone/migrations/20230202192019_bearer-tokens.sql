-- Add a column to `route` for an optional bearer token.
-- If the field is NULL, no bearer token is checked.

alter table "route" add column "bearer_token" varchar(255);
