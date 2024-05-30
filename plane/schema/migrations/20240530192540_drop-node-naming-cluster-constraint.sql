-- This index is: `unique index idx_cluster_name on node(cluster, name)`. We are
-- going to drop the `cluster` column, so we will re-introduce this constraint
-- but without the `cluster` column.
drop index idx_cluster_name;
