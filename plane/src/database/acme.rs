use plane_client::types::{ClusterName, NodeId};
use sqlx::query;
use sqlx::PgPool;

pub struct AcmeDatabase<'a> {
    pool: &'a PgPool,
}

impl<'a> AcmeDatabase<'a> {
    pub fn new(pool: &'a PgPool) -> Self {
        Self { pool }
    }

    pub async fn lease_cluster_dns(
        &self,
        cluster: &ClusterName,
        proxy: NodeId,
    ) -> sqlx::Result<bool> {
        let result = query!(
            r#"
            insert into acme_txt_entries (cluster, leased_at, leased_by)
            values ($1, now(), $2)
            on conflict (cluster)
            do update set
                leased_at = now(),
                leased_by = $2
            where (acme_txt_entries.leased_at + interval '1 minute') < now()
            "#,
            cluster.to_string(),
            proxy.as_i32(),
        )
        .execute(self.pool)
        .await?;

        Ok(result.rows_affected() == 1)
    }

    pub async fn set_cluster_dns(
        &self,
        cluster: &ClusterName,
        proxy: NodeId,
        txt_value: &str,
    ) -> sqlx::Result<bool> {
        let result = query!(
            r#"
            update acme_txt_entries set
                txt_value = $3
            where cluster = $1
            and leased_by = $2
            "#,
            cluster.to_string(),
            proxy.as_i32(),
            txt_value,
        )
        .execute(self.pool)
        .await?;

        Ok(result.rows_affected() == 1)
    }

    pub async fn txt_record_for_cluster(
        &self,
        cluster: &ClusterName,
    ) -> sqlx::Result<Option<String>> {
        let result = query!(
            r#"
            select txt_value
            from acme_txt_entries
            where cluster = $1
            "#,
            cluster.to_string(),
        )
        .fetch_optional(self.pool)
        .await?;

        Ok(result.and_then(|r| r.txt_value))
    }

    pub async fn release_cluster_lease(
        &self,
        cluster: &ClusterName,
        proxy: NodeId,
    ) -> sqlx::Result<()> {
        query!(
            r#"
            delete from acme_txt_entries
            where cluster = $1
            and leased_by = $2
            "#,
            cluster.to_string(),
            proxy.as_i32(),
        )
        .execute(self.pool)
        .await?;

        Ok(())
    }
}
