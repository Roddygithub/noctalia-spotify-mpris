//! Web API module for Spotify Web API with OAuth and SQLite cache

pub mod cache;
pub mod client;
pub mod responses;
pub mod types;

use axum::response::IntoResponse;
use axum::{routing::get, Router};
use std::sync::Arc;

pub use client::WebApiClient;

pub fn oauth_router(webapi: Arc<WebApiClient>) -> Router {
    Router::new()
        .route("/callback", get(oauth_callback))
        .route("/login", get(oauth_login))
        .with_state(webapi)
}

async fn oauth_login(
    axum::extract::State(webapi): axum::extract::State<Arc<WebApiClient>>,
) -> axum::response::Response {
    match webapi.auth_url().await {
        Ok(url) => axum::response::Redirect::to(&url).into_response(),
        Err(e) => format!(
            "Configuration error: {}.<br>Set SPOTIFY_CLIENT_ID and restart the backend.",
            e
        )
        .into_response(),
    }
}

async fn oauth_callback(
    axum::extract::State(webapi): axum::extract::State<Arc<WebApiClient>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl axum::response::IntoResponse {
    let code = params.get("code").cloned();
    let error = params.get("error").cloned();

    if let Some(error) = error {
        return format!("OAuth error: {}", error).into_response();
    }

    let code = match code {
        Some(c) => c,
        None => return "Missing code parameter".into_response(),
    };

    // Validate state to prevent CSRF
    if let Some(state) = params.get("state") {
        if !webapi.validate_state(state).await {
            return "Invalid state parameter (CSRF protection)".into_response();
        }
    }

    match webapi.exchange_code_for_token(&code).await {
        Ok(_) => "Authentication successful! You can close this window.".into_response(),
        Err(e) => format!("Authentication failed: {}", e).into_response(),
    }
}
