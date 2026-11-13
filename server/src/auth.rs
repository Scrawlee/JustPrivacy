//! Mots de passe, sessions, extracteur d'utilisateur, protection CSRF, IP client.
use crate::{crypto, error::{ApiError, ApiResult}, state::AppState};
use argon2::{password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString}, Argon2};
use axum::{
    extract::{ConnectInfo, FromRequestParts, Request, State},
    http::{header, request::Parts, HeaderMap, Method},
    middleware::Next,
    response::Response,
};
use std::{net::{IpAddr, SocketAddr}, sync::Arc};
use uuid::Uuid;

pub type Shared = Arc<AppState>;

const IDLE_DAYS: i64 = 14;

// ---------------------------------------------------------------- identifiants

const RESERVED: &[&str] = &["admin", "administrator", "root", "system", "support", "moderator", "justprivacy", "git", "api", "null", "undefined"];

pub fn normalize_username(raw: &str) -> ApiResult<String> {
    let u = raw.trim().to_lowercase();
    let n = u.chars().count();
    let ok_chars = u.chars().enumerate().all(|(i, c)| {
        c.is_ascii_lowercase() || c.is_ascii_digit() || (i > 0 && matches!(c, '.' | '_' | '-'))
    });
    if !(3..=32).contains(&n) || !ok_chars {
        return Err(ApiError::bad("invalid_username"));
    }
    if RESERVED.contains(&u.as_str()) {
        return Err(ApiError::bad("username_reserved"));
    }
    Ok(u)
}

pub fn check_password_policy(pw: &str, username: &str) -> ApiResult<()> {
    let n = pw.chars().count();
    if n < 10 { return Err(ApiError::bad("password_too_short")); }
    if n > 256 { return Err(ApiError::bad("password_too_long")); }
    if pw.to_lowercase() == username { return Err(ApiError::bad("password_is_username")); }
    Ok(())
}

// ------------------------------------------------------------------ Argon2id

pub fn new_hasher_hash(pw: &str) -> String {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default().hash_password(pw.as_bytes(), &salt).expect("argon2").to_string()
}

pub async fn hash_password(st: &Shared, pw: String) -> ApiResult<String> {
    let _permit = st.hash_slots.acquire().await.map_err(ApiError::internal)?;
    tokio::task::spawn_blocking(move || new_hasher_hash(&pw)).await.map_err(ApiError::internal)
}

pub async fn verify_password(st: &Shared, hash: String, pw: String) -> bool {
    let Ok(_permit) = st.hash_slots.acquire().await else { return false };
    tokio::task::spawn_blocking(move || {
        PasswordHash::new(&hash)
            .map(|h| Argon2::default().verify_password(pw.as_bytes(), &h).is_ok())
            .unwrap_or(false)
    })
    .await
    .unwrap_or(false)
}

// ------------------------------------------------------------------- sessions

pub async fn create_session(st: &Shared, user_id: Uuid) -> ApiResult<String> {
    let token = crypto::random_token();
    sqlx::query("INSERT INTO sessions (id, user_id, expires_at) VALUES ($1, $2, now() + make_interval(days => $3))")
        .bind(crypto::sha256(token.as_bytes()))
        .bind(user_id)
        .bind(IDLE_DAYS as i32)
        .execute(&st.db)
        .await?;
    Ok(token)
}

pub fn session_cookie(st: &AppState, token: &str) -> String {
    let secure = if st.cfg.secure_cookies() { "; Secure" } else { "" };
    format!("{}={}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}{}", st.cfg.session_cookie(), token, IDLE_DAYS * 86400, secure)
}

pub fn clear_cookie(st: &AppState) -> String {
    let secure = if st.cfg.secure_cookies() { "; Secure" } else { "" };
    format!("{}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{}", st.cfg.session_cookie(), secure)
}

fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v)
}

pub struct AuthUser {
    pub id: Uuid,
    pub username: String,
    pub role: String,
    pub session_id: Vec<u8>,
}

impl AuthUser {
    pub fn can_edit(&self) -> bool {
        matches!(self.role.as_str(), "editor" | "admin")
    }
}

impl FromRequestParts<Shared> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, st: &Shared) -> Result<Self, ApiError> {
        let token = cookie_value(&parts.headers, st.cfg.session_cookie()).ok_or(ApiError::unauthorized("unauthenticated"))?;
        let sid = crypto::sha256(token.as_bytes());
        let row: Option<(Uuid, String, String)> = sqlx::query_as(
            "SELECT u.id, u.username, u.role FROM sessions s JOIN users u ON u.id = s.user_id
             WHERE s.id = $1 AND s.expires_at > now() AND s.created_at > now() - interval '60 days'",
        )
        .bind(&sid)
        .fetch_optional(&st.db)
        .await?;
        let (id, username, role) = row.ok_or(ApiError::unauthorized("unauthenticated"))?;
        // Prolonge la session (au plus une fois par heure).
        sqlx::query("UPDATE sessions SET expires_at = now() + make_interval(days => $2) WHERE id = $1 AND expires_at < now() + make_interval(days => $2) - interval '1 hour'")
            .bind(&sid)
            .bind(IDLE_DAYS as i32)
            .execute(&st.db)
            .await?;
        Ok(AuthUser { id, username, role, session_id: sid })
    }
}

// ----------------------------------------------------------------------- CSRF

/// Les requêtes qui modifient l'état doivent venir de notre propre origine.
pub async fn csrf_guard(State(st): State<Shared>, req: Request, next: Next) -> Result<Response, ApiError> {
    if !matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS) {
        let h = req.headers();
        let own = st.cfg.public_url.origin().ascii_serialization();
        match h.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
            Some(o) if o == own => {}
            Some(_) => return Err(ApiError::forbidden("bad_origin")),
            None => {
                if let Some(site) = h.get("sec-fetch-site").and_then(|v| v.to_str().ok()) {
                    if !matches!(site, "same-origin" | "none") {
                        return Err(ApiError::forbidden("bad_origin"));
                    }
                }
            }
        }
    }
    Ok(next.run(req).await)
}

// ------------------------------------------------------------------ IP client

/// Adresse du client, pour la limitation de débit seulement (jamais stockée ni journalisée).
pub fn client_ip(headers: &HeaderMap, peer: &ConnectInfo<SocketAddr>, hops: usize) -> IpAddr {
    if hops > 0 {
        if let Some(xff) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
            let parts: Vec<&str> = xff.split(',').map(str::trim).collect();
            if parts.len() >= hops {
                if let Ok(ip) = parts[parts.len() - hops].parse() {
                    return ip;
                }
            }
        }
    }
    peer.0.ip()
}
