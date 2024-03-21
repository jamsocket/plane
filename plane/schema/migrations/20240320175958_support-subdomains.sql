alter table backend add column subdomain varchar(255);

comment on column backend.subdomain is 'Optional subdomain for session backend';

