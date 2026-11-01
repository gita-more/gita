//! 集群级设置（`vm_settings` 表，k/v）。
//!
//! 目前只有一个键：节点 API key。部署没有显式配置 key 时，控制面
//! 第一次启动生成随机值并写在这里，之后从数据库读——重启不换。

use anyhow::Result;
use sqlx::Row;

use crate::Db;

/// 节点代理访问控制面内部接口的共享 key。
pub const NODE_API_KEY: &str = "vm_node_api_key";

impl Db {
    pub async fn get_setting(&self, name: &str) -> Result<Option<String>> {
        let row = sqlx::query("SELECT value FROM vm_settings WHERE name = $1")
            .bind(name)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| r.try_get("value")).transpose()?)
    }

    /// 写入设置；不存在则插入，存在则覆盖（生成路径只在第一次走到）。
    pub async fn set_setting(&self, name: &str, value: &str, updated_at_ms: i64) -> Result<()> {
        sqlx::query(
            "INSERT INTO vm_settings (name, value, updated_at_ms)
             VALUES ($1, $2, $3)
             ON CONFLICT (name) DO UPDATE SET value = excluded.value,
                                              updated_at_ms = excluded.updated_at_ms",
        )
        .bind(name)
        .bind(value)
        .bind(updated_at_ms)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 插入设置；键已存在时**不**覆盖，返回是否真的插入了。
    ///
    /// 用于并发首启的“先写者赢”：两个控制面同时生成随机 key 时，
    /// 输的一方读到 false，回头 [`Self::get_setting`] 认领赢家的值，
    /// 保证全集群最终共享同一个 key。
    pub async fn insert_setting(&self, name: &str, value: &str, updated_at_ms: i64) -> Result<bool> {
        let res = sqlx::query(
            "INSERT INTO vm_settings (name, value, updated_at_ms)
             VALUES ($1, $2, $3)
             ON CONFLICT (name) DO NOTHING",
        )
        .bind(name)
        .bind(value)
        .bind(updated_at_ms)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_db() -> (Db, String) {
        let base = std::env::var("TEST_DATABASE_URL")
            .unwrap_or_else(|_| "postgres://gita:gita@localhost:55432/gita".into());
        let schema = format!(
            "gita_db_vm_settings_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let setup = sqlx::postgres::PgPoolOptions::new()
            .connect(&base)
            .await
            .unwrap();
        sqlx::query(&format!("CREATE SCHEMA \"{schema}\""))
            .execute(&setup)
            .await
            .unwrap();
        setup.close().await;

        let value = schema.clone();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .after_connect(move |c, _meta| {
                let schema = value.clone();
                Box::pin(async move {
                    sqlx::query(&format!("SET search_path TO \"{schema}\""))
                        .execute(c)
                        .await?;
                    Ok(())
                })
            })
            .connect(&base)
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        (Db::from_pool(pool), schema)
    }

    /// 幂等覆盖：写两次，读回最后一次的值。
    #[tokio::test]
    async fn set_then_get_overwrites() {
        let (db, _schema) = test_db().await;
        assert_eq!(db.get_setting(NODE_API_KEY).await.unwrap(), None);
        db.set_setting(NODE_API_KEY, "k1", 1).await.unwrap();
        db.set_setting(NODE_API_KEY, "k2", 2).await.unwrap();
        assert_eq!(
            db.get_setting(NODE_API_KEY).await.unwrap().as_deref(),
            Some("k2")
        );
    }

    /// 不同键互不影响（命名空间语义）。
    #[tokio::test]
    async fn distinct_names_are_isolated() {
        let (db, _schema) = test_db().await;
        db.set_setting("a", "va", 1).await.unwrap();
        assert_eq!(db.get_setting("b").await.unwrap(), None);
        assert_eq!(db.get_setting("a").await.unwrap().as_deref(), Some("va"));
    }

    /// 先写者赢：并发首启时第二个 insert 不覆盖第一个的值。
    #[tokio::test]
    async fn insert_keeps_the_first_value() {
        let (db, _schema) = test_db().await;
        assert!(db.insert_setting("k", "first", 1).await.unwrap());
        assert!(!db.insert_setting("k", "second", 2).await.unwrap());
        assert_eq!(db.get_setting("k").await.unwrap().as_deref(), Some("first"));
    }
}
