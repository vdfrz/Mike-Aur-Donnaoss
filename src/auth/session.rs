/// Opaque session tokens — UUID v4 stored in SQLite with TTL.
/// No JWT, no signing — all validation is a DB lookup.
use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use sqlx::SqlitePool;
use uuid::Uuid;

const SESSION_TTL_HOURS: i64 = 24 * 7; // 1 week for local use

#[derive(Debug, Clone)]
pub struct Session {
    pub token: String,
    pub user_id: String,
    pub expires_at: DateTime<Utc>,
}

/// In-memory + SQLite session store.
#[derive(Clone)]
pub struct SessionStore {
    db: SqlitePool,
}

impl SessionStore {
    pub fn new(db: SqlitePool) -> Self {
        Self { db }
    }

    pub async fn create(&self, user_id: &str) -> Result<String> {
        let token = Uuid::new_v4().to_string();
        let expires_at = Utc::now() + Duration::hours(SESSION_TTL_HOURS);
        sqlx::query(
            "INSERT INTO sessions (token, user_id, expires_at) VALUES (?, ?, ?)",
        )
        .bind(&token)
        .bind(user_id)
        .bind(expires_at.to_rfc3339())
        .execute(&self.db)
        .await?;
        Ok(token)
    }

    pub async fn validate(&self, token: &str) -> Result<Option<Session>> {
        let row: Option<(String, String, String)> = sqlx::query_as(
            "SELECT token, user_id, expires_at FROM sessions WHERE token = ?",
        )
        .bind(token)
        .fetch_optional(&self.db)
        .await?;

        let Some((tok, user_id, exp_str)) = row else {
            return Ok(None);
        };

        let expires_at = DateTime::parse_from_rfc3339(&exp_str)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or(Utc::now());

        if Utc::now() > expires_at {
            self.revoke(&tok).await?;
            return Ok(None);
        }

        Ok(Some(Session { token: tok, user_id, expires_at }))
    }

    pub async fn revoke(&self, token: &str) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE token = ?")
            .bind(token)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    pub async fn revoke_all(&self, user_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE user_id = ?")
            .bind(user_id)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// Purge expired sessions — call on startup or periodically.
    pub async fn purge_expired(&self) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE expires_at < ?")
            .bind(Utc::now().to_rfc3339())
            .execute(&self.db)
            .await?;
        Ok(())
    }
}
