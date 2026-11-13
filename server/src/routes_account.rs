//! Réglages du compte : mot de passe, A2F (OTP, passkeys), codes de secours, jetons git, sessions.
use crate::{
    auth::{self, check_password_policy, AuthUser, Shared},
    crypto,
    error::{ApiError, ApiResult},
    mfa,
    state::Ceremony,
};
use axum::{
    extract::{Path, State},
    http::header,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use rand::RngCore;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;
use uuid::Uuid;
use webauthn_rs::prelude::RegisterPublicKeyCredential;

pub fn router() -> Router<Shared> {
    Router::new()
        .route("/account/username", post(change_username))
        .route("/account/password", post(change_password))
        .route("/account/totp/setup", post(totp_setup))
        .route("/account/totp/enable", post(totp_enable))
        .route("/account/totp/disable", post(totp_disable))
        .route("/account/passkeys/start", post(passkey_start))
        .route("/account/passkeys/finish", post(passkey_finish))
        .route("/account/passkeys/{id}/delete", post(passkey_delete))
        .route("/account/recovery", post(recovery_regenerate))
        .route("/account/tokens", post(token_create))
        .route("/account/tokens/{id}/delete", post(token_delete))
        .route("/account/sessions/revoke-others", post(revoke_others))
        .route("/account/delete", post(delete_account))
}

#[derive(Deserialize)]
struct WithPassword { password: String }

/// Les actions sensibles redemandent le mot de passe, avec une limite de tentatives par compte.
async fn require_password(st: &Shared, user: &AuthUser, password: String) -> ApiResult<()> {
    if !st.limiter.hit(&format!("pw:{}", user.id), 10, Duration::from_secs(600)) {
        return Err(ApiError::too_many());
    }
    if password.chars().count() > 256 { return Err(ApiError::forbidden("wrong_password")); }
    let (hash,): (String,) = sqlx::query_as("SELECT password_hash FROM users WHERE id = $1").bind(user.id).fetch_one(&st.db).await?;
    if auth::verify_password(st, hash, password).await { Ok(()) } else { Err(ApiError::forbidden("wrong_password")) }
}

fn ok() -> Json<Value> { Json(json!({ "ok": true })) }

// ---------------------------------------------------------------- nom d'utilisateur

#[derive(Deserialize)]
struct ChangeUsername { username: String, password: String }

/// Changing only the capitals is free and instant. Changing the username itself is allowed once every 30 days;
/// the pending proposals follow the account to its new name.
async fn change_username(State(st): State<Shared>, user: AuthUser, Json(b): Json<ChangeUsername>) -> ApiResult<Json<Value>> {
    require_password(&st, &user, b.password).await?;
    let new = auth::normalize_username(&b.username)?;
    let display = auth::display_name(&b.username)?;
    if new == user.username {
        sqlx::query("UPDATE users SET display_name = $2 WHERE id = $1").bind(user.id).bind(&display).execute(&st.db).await?;
        return Ok(Json(json!({ "username": new, "display_name": display })));
    }
    let res = sqlx::query(
        "UPDATE users SET username = $2, display_name = $3, username_changed_at = now()
         WHERE id = $1 AND (username_changed_at IS NULL OR username_changed_at <= now() - interval '30 days')",
    )
    .bind(user.id)
    .bind(&new)
    .bind(&display)
    .execute(&st.db)
    .await;
    match res {
        Ok(r) if r.rows_affected() == 1 => {}
        Ok(_) => return Err(ApiError::conflict("username_cooldown")),
        Err(sqlx::Error::Database(e)) if e.code().as_deref() == Some("23505") => return Err(ApiError::conflict("username_taken")),
        Err(e) => return Err(e.into()),
    }
    let (git, old, new2) = (st.git.clone(), user.username.clone(), new.clone());
    let moved = tokio::task::spawn_blocking(move || git.rename_user_proposals(&old, &new2)).await.map_err(ApiError::internal)?;
    if let Err(e) = moved { tracing::error!("renommage des propositions : {e:?}"); }
    Ok(Json(json!({ "username": new, "display_name": display })))
}

// ------------------------------------------------------------------ mot de passe

#[derive(Deserialize)]
struct ChangePassword { current_password: String, new_password: String }

async fn change_password(State(st): State<Shared>, user: AuthUser, Json(b): Json<ChangePassword>) -> ApiResult<Json<Value>> {
    require_password(&st, &user, b.current_password).await?;
    check_password_policy(&b.new_password, &user.username)?;
    let hash = auth::hash_password(&st, b.new_password).await?;
    sqlx::query("UPDATE users SET password_hash = $2, password_changed_at = now() WHERE id = $1").bind(user.id).bind(hash).execute(&st.db).await?;
    // Les autres appareils doivent se reconnecter avec le nouveau mot de passe.
    sqlx::query("DELETE FROM sessions WHERE user_id = $1 AND id <> $2").bind(user.id).bind(&user.session_id).execute(&st.db).await?;
    Ok(ok())
}

// ------------------------------------------------------------------------- TOTP

async fn totp_setup(State(st): State<Shared>, user: AuthUser, Json(b): Json<WithPassword>) -> ApiResult<Json<Value>> {
    require_password(&st, &user, b.password).await?;
    let (enabled,): (bool,) = sqlx::query_as("SELECT totp_enabled FROM users WHERE id = $1").bind(user.id).fetch_one(&st.db).await?;
    if enabled { return Err(ApiError::conflict("totp_already_enabled")); }
    let mut secret = vec![0u8; 20];
    rand::rngs::OsRng.fill_bytes(&mut secret);
    sqlx::query("UPDATE users SET totp_pending = $2 WHERE id = $1").bind(user.id).bind(st.sealer.seal(&secret)).execute(&st.db).await?;
    let totp = mfa::new_totp(secret, &user.username)?;
    let url = totp.get_url();
    Ok(Json(json!({ "secret": totp.get_secret_base32(), "otpauth_url": url, "qr_svg": mfa::qr_svg(&url) })))
}

#[derive(Deserialize)]
struct Code { code: String }

async fn totp_enable(State(st): State<Shared>, user: AuthUser, Json(b): Json<Code>) -> ApiResult<Json<Value>> {
    if !st.limiter.hit(&format!("totp-enable:{}", user.id), 10, Duration::from_secs(600)) { return Err(ApiError::too_many()); }
    let (pending,): (Option<Vec<u8>>,) = sqlx::query_as("SELECT totp_pending FROM users WHERE id = $1").bind(user.id).fetch_one(&st.db).await?;
    let secret = pending.and_then(|p| st.sealer.open(&p)).ok_or(ApiError::bad("totp_not_started"))?;
    let totp = mfa::new_totp(secret, &user.username)?;
    let step = mfa::match_step(&totp, &b.code, 0).ok_or(ApiError::bad("invalid_code"))?;
    sqlx::query("UPDATE users SET totp_secret = totp_pending, totp_pending = NULL, totp_enabled = TRUE, totp_last_step = $2 WHERE id = $1")
        .bind(user.id)
        .bind(step)
        .execute(&st.db)
        .await?;
    let mut out = json!({ "ok": true });
    if mfa::recovery_count(&st, user.id).await? == 0 {
        out["recovery_codes"] = json!(mfa::regenerate_recovery_codes(&st, user.id).await?);
    }
    Ok(Json(out))
}

async fn totp_disable(State(st): State<Shared>, user: AuthUser, Json(b): Json<WithPassword>) -> ApiResult<Json<Value>> {
    require_password(&st, &user, b.password).await?;
    sqlx::query("UPDATE users SET totp_secret = NULL, totp_pending = NULL, totp_enabled = FALSE, totp_last_step = 0 WHERE id = $1").bind(user.id).execute(&st.db).await?;
    mfa::drop_recovery_if_unneeded(&st, user.id).await?;
    Ok(ok())
}

// --------------------------------------------------------------------- passkeys

const MAX_PASSKEYS: usize = 10;

async fn passkey_start(State(st): State<Shared>, user: AuthUser) -> ApiResult<Json<Value>> {
    let existing = mfa::load_passkeys(&st, user.id).await?;
    if existing.len() >= MAX_PASSKEYS { return Err(ApiError::conflict("too_many_passkeys")); }
    let exclude = existing.iter().map(|(_, p)| p.cred_id().clone()).collect::<Vec<_>>();
    let (challenge, state) = st
        .webauthn
        .start_passkey_registration(user.id, &user.username, &user.username, Some(exclude))
        .map_err(ApiError::internal)?;
    st.put_ceremony(format!("reg:{}", user.id), Ceremony::Register(state));
    Ok(Json(serde_json::to_value(challenge).map_err(ApiError::internal)?))
}

#[derive(Deserialize)]
struct PasskeyFinish { credential: RegisterPublicKeyCredential, #[serde(default)] name: String }

async fn passkey_finish(State(st): State<Shared>, user: AuthUser, Json(b): Json<PasskeyFinish>) -> ApiResult<Json<Value>> {
    let Some(Ceremony::Register(state)) = st.take_ceremony(&format!("reg:{}", user.id)) else {
        return Err(ApiError::bad("ceremony_expired"));
    };
    let key = st.webauthn.finish_passkey_registration(&b.credential, &state).map_err(|e| {
        tracing::info!("enregistrement de passkey refusé : {e}");
        ApiError::bad("invalid_passkey")
    })?;
    let name: String = b.name.trim().chars().take(64).collect();
    let name = if name.is_empty() { "Passkey".to_string() } else { name };
    sqlx::query("INSERT INTO passkeys (user_id, name, credential) VALUES ($1, $2, $3)")
        .bind(user.id)
        .bind(name)
        .bind(serde_json::to_value(&key).map_err(ApiError::internal)?)
        .execute(&st.db)
        .await?;
    let mut out = json!({ "ok": true });
    if mfa::recovery_count(&st, user.id).await? == 0 {
        out["recovery_codes"] = json!(mfa::regenerate_recovery_codes(&st, user.id).await?);
    }
    Ok(Json(out))
}

async fn passkey_delete(State(st): State<Shared>, user: AuthUser, Path(id): Path<Uuid>, Json(b): Json<WithPassword>) -> ApiResult<Json<Value>> {
    require_password(&st, &user, b.password).await?;
    let done = sqlx::query("DELETE FROM passkeys WHERE id = $1 AND user_id = $2").bind(id).bind(user.id).execute(&st.db).await?;
    if done.rows_affected() == 0 { return Err(ApiError::not_found()); }
    mfa::drop_recovery_if_unneeded(&st, user.id).await?;
    Ok(ok())
}

// ------------------------------------------------------------ codes de secours

async fn recovery_regenerate(State(st): State<Shared>, user: AuthUser, Json(b): Json<WithPassword>) -> ApiResult<Json<Value>> {
    require_password(&st, &user, b.password).await?;
    if !mfa::has_second_factor(&st, user.id).await? { return Err(ApiError::conflict("no_second_factor")); }
    Ok(Json(json!({ "recovery_codes": mfa::regenerate_recovery_codes(&st, user.id).await? })))
}

// ------------------------------------------------------------------ jetons git

#[derive(Deserialize)]
struct NewToken { password: String, name: String, #[serde(default)] expires_days: Option<i32> }

async fn token_create(State(st): State<Shared>, user: AuthUser, Json(b): Json<NewToken>) -> ApiResult<Json<Value>> {
    require_password(&st, &user, b.password).await?;
    let name: String = b.name.trim().chars().take(64).collect();
    if name.is_empty() { return Err(ApiError::bad("invalid_name")); }
    if matches!(b.expires_days, Some(d) if !(1..=365).contains(&d)) { return Err(ApiError::bad("invalid_expiry")); }
    let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM api_tokens WHERE user_id = $1").bind(user.id).fetch_one(&st.db).await?;
    if count >= 20 { return Err(ApiError::conflict("too_many_tokens")); }
    let token = format!("jpt_{}", crypto::random_token());
    sqlx::query("INSERT INTO api_tokens (user_id, name, token_hash, expires_at) VALUES ($1, $2, $3, CASE WHEN $4::int IS NULL THEN NULL ELSE now() + make_interval(days => $4::int) END)")
        .bind(user.id)
        .bind(name)
        .bind(crypto::sha256(token.as_bytes()))
        .bind(b.expires_days)
        .execute(&st.db)
        .await?;
    Ok(Json(json!({ "token": token })))
}

async fn token_delete(State(st): State<Shared>, user: AuthUser, Path(id): Path<Uuid>) -> ApiResult<Json<Value>> {
    let done = sqlx::query("DELETE FROM api_tokens WHERE id = $1 AND user_id = $2").bind(id).bind(user.id).execute(&st.db).await?;
    if done.rows_affected() == 0 { return Err(ApiError::not_found()); }
    Ok(ok())
}

// ------------------------------------------------------------- sessions / compte

async fn revoke_others(State(st): State<Shared>, user: AuthUser) -> ApiResult<Json<Value>> {
    sqlx::query("DELETE FROM sessions WHERE user_id = $1 AND id <> $2").bind(user.id).bind(&user.session_id).execute(&st.db).await?;
    Ok(ok())
}

async fn delete_account(State(st): State<Shared>, user: AuthUser, Json(b): Json<WithPassword>) -> ApiResult<Response> {
    require_password(&st, &user, b.password).await?;
    sqlx::query("DELETE FROM users WHERE id = $1").bind(user.id).execute(&st.db).await?;
    Ok(([(header::SET_COOKIE, auth::clear_cookie(&st))], ok()).into_response())
}
