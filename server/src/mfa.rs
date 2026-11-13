//! TOTP, codes de secours et passkeys : primitives partagées par la connexion et les réglages.
use crate::{auth::Shared, crypto, error::{ApiError, ApiResult}};
use qrcodegen::{QrCode, QrCodeEcc};
use rand::Rng;
use subtle::ConstantTimeEq;
use totp_rs::{Algorithm, TOTP};
use uuid::Uuid;
use webauthn_rs::prelude::{AuthenticationResult, Passkey};

pub const PERIOD: u64 = 30;

pub fn new_totp(secret: Vec<u8>, username: &str) -> ApiResult<TOTP> {
    TOTP::new(Algorithm::SHA1, 6, 0, PERIOD, secret, Some("JustPrivacy".into()), username.into()).map_err(ApiError::internal)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

/// Cherche le pas de 30 s (±1) dont le code correspond ; refuse tout pas déjà utilisé.
pub fn match_step(totp: &TOTP, code: &str, last_step: i64) -> Option<i64> {
    let code = code.trim().replace(' ', "");
    if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) { return None; }
    let step = (now_secs() / PERIOD) as i64;
    for s in [step - 1, step, step + 1] {
        let expected = totp.generate((s as u64) * PERIOD);
        if bool::from(expected.as_bytes().ct_eq(code.as_bytes())) && s > last_step {
            return Some(s);
        }
    }
    None
}

/// Vérifie un code TOTP à 6 chiffres, sinon un code de secours (usage unique).
pub async fn check_totp_or_recovery(st: &Shared, user_id: Uuid, input: &str) -> ApiResult<bool> {
    #[derive(sqlx::FromRow)]
    struct U { username: String, totp_secret: Option<Vec<u8>>, totp_enabled: bool, totp_last_step: i64 }
    let u: U = sqlx::query_as("SELECT username, totp_secret, totp_enabled, totp_last_step FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(&st.db)
        .await?;
    let compact: String = input.chars().filter(|c| !c.is_whitespace() && *c != '-').collect();
    if compact.len() == 6 && compact.bytes().all(|b| b.is_ascii_digit()) {
        if !u.totp_enabled { return Ok(false); }
        let Some(secret) = u.totp_secret.as_deref().and_then(|s| st.sealer.open(s)) else { return Ok(false) };
        let totp = new_totp(secret, &u.username)?;
        let Some(step) = match_step(&totp, &compact, u.totp_last_step) else { return Ok(false) };
        // Mise à jour conditionnelle : un même code ne peut pas servir deux fois, même en parallèle.
        let done = sqlx::query("UPDATE users SET totp_last_step = $2 WHERE id = $1 AND totp_last_step < $2")
            .bind(user_id)
            .bind(step)
            .execute(&st.db)
            .await?;
        return Ok(done.rows_affected() == 1);
    }
    let done = sqlx::query("UPDATE recovery_codes SET used_at = now() WHERE user_id = $1 AND code_hash = $2 AND used_at IS NULL")
        .bind(user_id)
        .bind(crypto::sha256(compact.to_lowercase().as_bytes()))
        .execute(&st.db)
        .await?;
    Ok(done.rows_affected() == 1)
}

// ----------------------------------------------------------- codes de secours

const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz234567";

/// Remplace les codes de secours par 10 nouveaux (80 bits chacun) et les renvoie en clair, une seule fois.
pub async fn regenerate_recovery_codes(st: &Shared, user_id: Uuid) -> ApiResult<Vec<String>> {
    let mut rng = rand::rngs::OsRng;
    let codes: Vec<String> = (0..10)
        .map(|_| {
            let raw: String = (0..16).map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char).collect();
            format!("{}-{}-{}-{}", &raw[0..4], &raw[4..8], &raw[8..12], &raw[12..16])
        })
        .collect();
    let mut tx = st.db.begin().await?;
    sqlx::query("DELETE FROM recovery_codes WHERE user_id = $1").bind(user_id).execute(&mut *tx).await?;
    for c in &codes {
        sqlx::query("INSERT INTO recovery_codes (user_id, code_hash) VALUES ($1, $2)")
            .bind(user_id)
            .bind(crypto::sha256(c.replace('-', "").as_bytes()))
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(codes)
}

pub async fn has_second_factor(st: &Shared, user_id: Uuid) -> ApiResult<bool> {
    let (n,): (i64,) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM passkeys WHERE user_id = $1) + (SELECT count(*) FROM users WHERE id = $1 AND totp_enabled)",
    )
    .bind(user_id)
    .fetch_one(&st.db)
    .await?;
    Ok(n > 0)
}

/// Sans second facteur, les codes de secours n'ont plus de sens.
pub async fn drop_recovery_if_unneeded(st: &Shared, user_id: Uuid) -> ApiResult<()> {
    if !has_second_factor(st, user_id).await? {
        sqlx::query("DELETE FROM recovery_codes WHERE user_id = $1").bind(user_id).execute(&st.db).await?;
    }
    Ok(())
}

pub async fn recovery_count(st: &Shared, user_id: Uuid) -> ApiResult<i64> {
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM recovery_codes WHERE user_id = $1").bind(user_id).fetch_one(&st.db).await?;
    Ok(n)
}

// --------------------------------------------------------------------- passkeys

pub async fn load_passkeys(st: &Shared, user_id: Uuid) -> ApiResult<Vec<(Uuid, Passkey)>> {
    let rows: Vec<(Uuid, serde_json::Value)> = sqlx::query_as("SELECT id, credential FROM passkeys WHERE user_id = $1").bind(user_id).fetch_all(&st.db).await?;
    rows.into_iter()
        .map(|(id, v)| serde_json::from_value(v).map(|p| (id, p)).map_err(ApiError::internal))
        .collect()
}

/// Met à jour le compteur de signatures de la passkey utilisée.
pub async fn record_passkey_use(st: &Shared, user_id: Uuid, result: &AuthenticationResult) -> ApiResult<()> {
    for (id, mut key) in load_passkeys(st, user_id).await? {
        if key.cred_id() == result.cred_id() {
            key.update_credential(result);
            sqlx::query("UPDATE passkeys SET credential = $2, last_used_at = now() WHERE id = $1")
                .bind(id)
                .bind(serde_json::to_value(&key).map_err(ApiError::internal)?)
                .execute(&st.db)
                .await?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------- QR code

/// QR code en SVG (noir sur blanc, pour rester lisible quel que soit le thème).
pub fn qr_svg(text: &str) -> String {
    let Ok(qr) = QrCode::encode_text(text, QrCodeEcc::Medium) else { return String::new() };
    let border = 2;
    let n = qr.size();
    let mut path = String::new();
    for y in 0..n {
        for x in 0..n {
            if qr.get_module(x, y) {
                path.push_str(&format!("M{},{}h1v1h-1z", x + border, y + border));
            }
        }
    }
    let dim = n + border * 2;
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {dim} {dim}\" shape-rendering=\"crispEdges\" role=\"img\" aria-label=\"QR code\"><rect width=\"{dim}\" height=\"{dim}\" fill=\"#fff\"/><path d=\"{path}\" fill=\"#000\"/></svg>"
    )
}
