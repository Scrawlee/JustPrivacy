//! Inscription, connexion (mot de passe + second facteur), déconnexion.
use crate::{
    auth::{self, check_password_policy, client_ip, normalize_username, AuthUser, Shared},
    captcha, crypto,
    error::{ApiError, ApiResult},
    mfa,
    state::Ceremony,
};
use axum::{
    extract::{ConnectInfo, State},
    http::{header, HeaderMap},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{net::{IpAddr, SocketAddr}, time::Duration};
use uuid::Uuid;
use webauthn_rs::prelude::{Passkey, PublicKeyCredential};

pub fn router() -> Router<Shared> {
    Router::new()
        .route("/auth/signup", post(signup))
        .route("/auth/login", post(login))
        .route("/auth/mfa/totp", post(mfa_totp))
        .route("/auth/mfa/passkey/start", post(mfa_passkey_start))
        .route("/auth/mfa/passkey/finish", post(mfa_passkey_finish))
        .route("/auth/logout", post(logout))
        .route("/me", get(me))
}

/// Les adresses IPv6 sont regroupées par /64 pour la limitation de débit.
fn ip_key(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => {
            let s = v6.segments();
            format!("{:x}:{:x}:{:x}:{:x}", s[0], s[1], s[2], s[3])
        }
    }
}

fn limit(st: &Shared, key: String, max: u32, window: Duration) -> ApiResult<()> {
    if st.limiter.hit(&key, max, window) { Ok(()) } else { Err(ApiError::too_many()) }
}

async fn start_session(st: &Shared, user_id: Uuid, username: &str, role: &str) -> ApiResult<Response> {
    let token = auth::create_session(st, user_id).await?;
    Ok((
        [(header::SET_COOKIE, auth::session_cookie(st, &token))],
        Json(json!({ "user": { "username": username, "role": role } })),
    )
        .into_response())
}

// ------------------------------------------------------------------ inscription

#[derive(Deserialize)]
struct Credentials {
    username: String,
    password: String,
    #[serde(default)]
    cap_token: String,
}

async fn signup(
    State(st): State<Shared>,
    headers: HeaderMap,
    peer: ConnectInfo<SocketAddr>,
    Json(b): Json<Credentials>,
) -> ApiResult<Response> {
    let ip = ip_key(client_ip(&headers, &peer, st.cfg.trust_proxy_hops));
    limit(&st, format!("signup:{ip}"), 5, Duration::from_secs(3600))?;
    let username = normalize_username(&b.username)?;
    check_password_policy(&b.password, &username)?;
    captcha::verify(&st, &b.cap_token).await?;
    let hash = auth::hash_password(&st, b.password).await?;
    let id: Option<(Uuid,)> = sqlx::query_as(
        "INSERT INTO users (username, password_hash) VALUES ($1, $2) ON CONFLICT (username) DO NOTHING RETURNING id",
    )
    .bind(&username)
    .bind(&hash)
    .fetch_optional(&st.db)
    .await?;
    let (id,) = id.ok_or(ApiError::conflict("username_taken"))?;
    start_session(&st, id, &username, "user").await
}

// -------------------------------------------------------------------- connexion

const MAX_FAILURES: i32 = 8;

async fn login(
    State(st): State<Shared>,
    headers: HeaderMap,
    peer: ConnectInfo<SocketAddr>,
    Json(b): Json<Credentials>,
) -> ApiResult<Response> {
    let ip = ip_key(client_ip(&headers, &peer, st.cfg.trust_proxy_hops));
    limit(&st, format!("login:{ip}"), 20, Duration::from_secs(600))?;
    let username = b.username.trim().to_lowercase();
    if username.len() > 64 || b.password.chars().count() > 256 {
        return Err(ApiError::bad("invalid_credentials"));
    }
    captcha::verify(&st, &b.cap_token).await?;

    #[derive(sqlx::FromRow)]
    struct Row { id: Uuid, password_hash: String, role: String, totp_enabled: bool, locked: bool }
    let user: Option<Row> = sqlx::query_as(
        "SELECT id, password_hash, role, totp_enabled, COALESCE(locked_until > now(), FALSE) AS locked FROM users WHERE username = $1",
    )
    .bind(&username)
    .fetch_optional(&st.db)
    .await?;

    // Compte inconnu : on hache quand même pour ne pas révéler son existence par le temps de réponse.
    let Some(user) = user else {
        let dummy = st.dummy_hash.clone();
        auth::verify_password(&st, dummy, b.password).await;
        return Err(ApiError::unauthorized("invalid_credentials"));
    };
    let ok = auth::verify_password(&st, user.password_hash.clone(), b.password).await;
    if user.locked {
        return Err(ApiError::new(axum::http::StatusCode::TOO_MANY_REQUESTS, "account_locked"));
    }
    if !ok {
        sqlx::query(
            "UPDATE users SET failed_attempts = CASE WHEN failed_attempts + 1 >= $2 THEN 0 ELSE failed_attempts + 1 END,
                              locked_until = CASE WHEN failed_attempts + 1 >= $2 THEN now() + interval '10 minutes' ELSE locked_until END
             WHERE id = $1",
        )
        .bind(user.id)
        .bind(MAX_FAILURES)
        .execute(&st.db)
        .await?;
        return Err(ApiError::unauthorized("invalid_credentials"));
    }
    sqlx::query("UPDATE users SET failed_attempts = 0, locked_until = NULL WHERE id = $1")
        .bind(user.id)
        .execute(&st.db)
        .await?;

    let passkeys: (i64,) = sqlx::query_as("SELECT count(*) FROM passkeys WHERE user_id = $1").bind(user.id).fetch_one(&st.db).await?;
    if user.totp_enabled || passkeys.0 > 0 {
        let ticket = crypto::random_token();
        sqlx::query("INSERT INTO pending_logins (id, user_id, expires_at) VALUES ($1, $2, now() + interval '5 minutes')")
            .bind(crypto::sha256(ticket.as_bytes()))
            .bind(user.id)
            .execute(&st.db)
            .await?;
        sqlx::query("DELETE FROM pending_logins WHERE expires_at < now()").execute(&st.db).await?;
        return Ok(Json(json!({ "mfa": { "ticket": ticket, "totp": user.totp_enabled, "passkey": passkeys.0 > 0 } })).into_response());
    }
    start_session(&st, user.id, &username, &user.role).await
}

// ------------------------------------------------------------ second facteur

#[derive(Deserialize)]
struct TotpStep { ticket: String, code: String }

/// Consomme une tentative sur le ticket (5 au maximum) et renvoie l'utilisateur.
async fn use_ticket(st: &Shared, ticket: &str) -> ApiResult<Uuid> {
    let row: Option<(Uuid,)> = sqlx::query_as(
        "UPDATE pending_logins SET attempts = attempts + 1 WHERE id = $1 AND expires_at > now() AND attempts < 5 RETURNING user_id",
    )
    .bind(crypto::sha256(ticket.as_bytes()))
    .fetch_optional(&st.db)
    .await?;
    row.map(|r| r.0).ok_or(ApiError::unauthorized("ticket_expired"))
}

async fn finish_mfa(st: &Shared, ticket: &str, user_id: Uuid) -> ApiResult<Response> {
    sqlx::query("DELETE FROM pending_logins WHERE id = $1").bind(crypto::sha256(ticket.as_bytes())).execute(&st.db).await?;
    let (username, role): (String, String) = sqlx::query_as("SELECT username, role FROM users WHERE id = $1").bind(user_id).fetch_one(&st.db).await?;
    start_session(st, user_id, &username, &role).await
}

async fn mfa_totp(State(st): State<Shared>, Json(b): Json<TotpStep>) -> ApiResult<Response> {
    if b.code.len() > 64 { return Err(ApiError::bad("invalid_code")); }
    let user_id = use_ticket(&st, &b.ticket).await?;
    if !mfa::check_totp_or_recovery(&st, user_id, &b.code).await? {
        return Err(ApiError::unauthorized("invalid_code"));
    }
    finish_mfa(&st, &b.ticket, user_id).await
}

#[derive(Deserialize)]
struct Ticket { ticket: String }

async fn mfa_passkey_start(State(st): State<Shared>, Json(b): Json<Ticket>) -> ApiResult<Json<Value>> {
    let row: Option<(Uuid,)> = sqlx::query_as("SELECT user_id FROM pending_logins WHERE id = $1 AND expires_at > now() AND attempts < 5")
        .bind(crypto::sha256(b.ticket.as_bytes()))
        .fetch_optional(&st.db)
        .await?;
    let (user_id,) = row.ok_or(ApiError::unauthorized("ticket_expired"))?;
    let keys = mfa::load_passkeys(&st, user_id).await?;
    if keys.is_empty() { return Err(ApiError::bad("no_passkey")); }
    let list: Vec<Passkey> = keys.into_iter().map(|(_, p)| p).collect();
    let (challenge, state) = st.webauthn.start_passkey_authentication(&list).map_err(ApiError::internal)?;
    st.put_ceremony(format!("login:{}", hex::encode(crypto::sha256(b.ticket.as_bytes()))), Ceremony::Login(state));
    Ok(Json(serde_json::to_value(challenge).map_err(ApiError::internal)?))
}

#[derive(Deserialize)]
struct PasskeyFinish { ticket: String, credential: PublicKeyCredential }

async fn mfa_passkey_finish(State(st): State<Shared>, Json(b): Json<PasskeyFinish>) -> ApiResult<Response> {
    let user_id = use_ticket(&st, &b.ticket).await?;
    let Some(Ceremony::Login(state)) = st.take_ceremony(&format!("login:{}", hex::encode(crypto::sha256(b.ticket.as_bytes())))) else {
        return Err(ApiError::unauthorized("ticket_expired"));
    };
    let result = st.webauthn.finish_passkey_authentication(&b.credential, &state).map_err(|e| {
        tracing::info!("passkey refusée : {e}");
        ApiError::unauthorized("invalid_passkey")
    })?;
    mfa::record_passkey_use(&st, user_id, &result).await?;
    finish_mfa(&st, &b.ticket, user_id).await
}

// --------------------------------------------------------------- déconnexion / profil

async fn logout(State(st): State<Shared>, user: AuthUser) -> ApiResult<Response> {
    sqlx::query("DELETE FROM sessions WHERE id = $1").bind(&user.session_id).execute(&st.db).await?;
    Ok(([(header::SET_COOKIE, auth::clear_cookie(&st))], Json(json!({ "ok": true }))).into_response())
}

async fn me(State(st): State<Shared>, user: AuthUser) -> ApiResult<Json<Value>> {
    let (totp,): (bool,) = sqlx::query_as("SELECT totp_enabled FROM users WHERE id = $1").bind(user.id).fetch_one(&st.db).await?;
    let passkeys: Vec<(Uuid, String, chrono::DateTime<chrono::Utc>, Option<chrono::DateTime<chrono::Utc>>)> =
        sqlx::query_as("SELECT id, name, created_at, last_used_at FROM passkeys WHERE user_id = $1 ORDER BY created_at").bind(user.id).fetch_all(&st.db).await?;
    let tokens: Vec<(Uuid, String, chrono::DateTime<chrono::Utc>, Option<chrono::DateTime<chrono::Utc>>, Option<chrono::DateTime<chrono::Utc>>)> =
        sqlx::query_as("SELECT id, name, created_at, last_used_at, expires_at FROM api_tokens WHERE user_id = $1 ORDER BY created_at").bind(user.id).fetch_all(&st.db).await?;
    let (recovery,): (i64,) = sqlx::query_as("SELECT count(*) FROM recovery_codes WHERE user_id = $1 AND used_at IS NULL").bind(user.id).fetch_one(&st.db).await?;
    let (sessions,): (i64,) = sqlx::query_as("SELECT count(*) FROM sessions WHERE user_id = $1 AND expires_at > now()").bind(user.id).fetch_one(&st.db).await?;
    Ok(Json(json!({
        "username": user.username,
        "role": user.role,
        "totp_enabled": totp,
        "passkeys": passkeys.iter().map(|p| json!({ "id": p.0, "name": p.1, "created_at": p.2, "last_used_at": p.3 })).collect::<Vec<_>>(),
        "tokens": tokens.iter().map(|t| json!({ "id": t.0, "name": t.1, "created_at": t.2, "last_used_at": t.3, "expires_at": t.4 })).collect::<Vec<_>>(),
        "recovery_remaining": recovery,
        "sessions": sessions,
        "git_url": format!("{}/git/articles.git", st.cfg.public_url.as_str().trim_end_matches('/')),
    })))
}
