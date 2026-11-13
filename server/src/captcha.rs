use crate::{error::{ApiError, ApiResult}, state::AppState};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
struct SiteVerify {
    success: bool,
}

/// Vérifie un jeton Cap auprès du serveur Cap (/siteverify). Un jeton ne sert qu'une fois.
pub async fn verify(state: &AppState, token: &str) -> ApiResult<()> {
    if state.cfg.cap_disabled {
        return Ok(());
    }
    if token.is_empty() || token.len() > 2048 {
        return Err(ApiError::bad("captcha_required"));
    }
    let secret = state.cfg.cap_secret.as_deref().unwrap_or_default();
    let resp = state
        .http
        .post(&state.cfg.cap_siteverify_url)
        .json(&json!({ "secret": secret, "response": token }))
        .send()
        .await
        .map_err(|e| {
            tracing::warn!("Cap injoignable : {e}");
            ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "captcha_unavailable")
        })?;
    // Le serveur Cap répond 200 avec success=false pour un jeton invalide.
    let body: SiteVerify = resp.json().await.map_err(|e| {
        tracing::warn!("réponse Cap illisible : {e}");
        ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "captcha_unavailable")
    })?;
    if body.success { Ok(()) } else { Err(ApiError::bad("captcha_failed")) }
}
