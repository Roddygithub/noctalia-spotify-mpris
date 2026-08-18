//! SQLite cache for OAuth tokens and API responses

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use sqlx::{Pool, Sqlite, Row};
use uuid::Uuid;

use crate::webapi::types::{CachedResponse, TokenResponse};

/// Token cache key
const TOKEN_CACHE_KEY: &str = "spotify_oauth_token";

pub struct Cache {
    pool: Pool<Sqlite>,
}

impl Cache {
    pub async fn new(pool: Pool<Sqlite>) -> Result<Self> {
        let cache = Self { pool };
        cache.init().await?;
        Ok(cache)
    }

    async fn init(&self) -> Result<()> {
        // Token table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS oauth_tokens (
                key TEXT PRIMARY KEY,
                access_token TEXT NOT NULL,
                token_type TEXT NOT NULL,
                expires_in INTEGER NOT NULL,
                refresh_token TEXT,
                scope TEXT NOT NULL,
                obtained_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // API response cache table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS api_cache (
                id TEXT PRIMARY KEY,
                endpoint TEXT NOT NULL,
                params_hash TEXT NOT NULL,
                response_json TEXT NOT NULL,
                expires_at TEXT NOT NULL,
                created_at TEXT NOT NULL,
                UNIQUE(endpoint, params_hash)
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Index for cleanup
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_api_cache_expires_at ON api_cache(expires_at)
            "#,
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Get cached OAuth token
    pub async fn get_token(&self) -> Result<Option<TokenResponse>> {
        let row = sqlx::query(
            "SELECT access_token, token_type, expires_in, refresh_token, scope, obtained_at FROM oauth_tokens WHERE key = ?"
        )
        .bind(TOKEN_CACHE_KEY)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(row) = row {
            let obtained_at: String = row.get("obtained_at");
            let obtained_at = DateTime::parse_from_rfc3339(&obtained_at)?.with_timezone(&Utc);

            Ok(Some(TokenResponse {
                access_token: row.get("access_token"),
                token_type: row.get("token_type"),
                expires_in: row.get("expires_in"),
                refresh_token: row.get("refresh_token"),
                scope: row.get("scope"),
                obtained_at: Some(obtained_at),
            }))
        } else {
            Ok(None)
        }
    }

    /// Store OAuth token
    pub async fn set_token(&self, token: &TokenResponse) -> Result<()> {
        let obtained_at = token.obtained_at.unwrap_or_else(Utc::now).to_rfc3339();

        sqlx::query(
            r#"
            INSERT INTO oauth_tokens (key, access_token, token_type, expires_in, refresh_token, scope, obtained_at)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(key) DO UPDATE SET
                access_token = excluded.access_token,
                token_type = excluded.token_type,
                expires_in = excluded.expires_in,
                refresh_token = excluded.refresh_token,
                scope = excluded.scope,
                obtained_at = excluded.obtained_at
            "#,
        )
        .bind(TOKEN_CACHE_KEY)
        .bind(&token.access_token)
        .bind(&token.token_type)
        .bind(token.expires_in)
        .bind(&token.refresh_token)
        .bind(&token.scope)
        .bind(&obtained_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Clear OAuth token
    pub async fn clear_token(&self) -> Result<()> {
        sqlx::query("DELETE FROM oauth_tokens WHERE key = ?")
            .bind(TOKEN_CACHE_KEY)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Get cached API response
    pub async fn get(&self, endpoint: &str, params_hash: &str) -> Result<Option<String>> {
        let row = sqlx::query(
            "SELECT response_json FROM api_cache WHERE endpoint = ? AND params_hash = ? AND expires_at > ?"
        )
        .bind(endpoint)
        .bind(params_hash)
        .bind(Utc::now().to_rfc3339())
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| r.get("response_json")))
    }

    /// Store API response in cache
    pub async fn set(&self, endpoint: &str, params_hash: &str, response_json: &str, ttl_seconds: i64) -> Result<()> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        let expires_at = now + Duration::seconds(ttl_seconds);

        sqlx::query(
            r#"
            INSERT INTO api_cache (id, endpoint, params_hash, response_json, expires_at, created_at)
            VALUES (?, ?, ?, ?, ?, ?)
            ON CONFLICT(endpoint, params_hash) DO UPDATE SET
                response_json = excluded.response_json,
                expires_at = excluded.expires_at,
                created_at = excluded.created_at
            "#,
        )
        .bind(&id)
        .bind(endpoint)
        .bind(params_hash)
        .bind(response_json)
        .bind(expires_at.to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Clean expired cache entries
    pub async fn cleanup(&self) -> Result<u64> {
        let result = sqlx::query("DELETE FROM api_cache WHERE expires_at <= ?")
            .bind(Utc::now().to_rfc3339())
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }
}