-- we don't query the backends by cluster and drone_id anymore, just by drone_id
drop index idx_backend_drone_id;
create index idx_backend_drone_id on backend(drone_id) where last_status != 'terminated';
