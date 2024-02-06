-- Statuses are now lowercase, so we need to update the index condition to reflect that.

drop index idx_backend_drone_id;
create index idx_backend_drone_id on backend(cluster, drone_id) where last_status != 'terminated';
