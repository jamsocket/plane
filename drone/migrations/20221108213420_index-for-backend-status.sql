-- Add an index for drone state, so that counting the number of live drones
-- does not become (much) more expensive as the number of non-live drones grows.

create index "state" on "backend" ( "state" );
