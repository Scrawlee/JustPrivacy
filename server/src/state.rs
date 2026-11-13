use crate::{config::Config, crypto::Sealer, gitrepo::GitStore, pages::PageCache, ratelimit::RateLimiter};
use sqlx::PgPool;
use std::{collections::HashMap, sync::{Arc, Mutex}, time::{Duration, Instant}};
use tokio::sync::Semaphore;
use webauthn_rs::prelude::{PasskeyAuthentication, PasskeyRegistration, Webauthn};

/// Cérémonies WebAuthn en cours (mémoire uniquement, 5 minutes de validité).
pub enum Ceremony {
    Register(PasskeyRegistration),
    Login(PasskeyAuthentication),
}

pub struct AppState {
    pub cfg: Config,
    pub db: PgPool,
    pub webauthn: Webauthn,
    pub sealer: Sealer,
    pub http: reqwest::Client,
    pub limiter: RateLimiter,
    /// Borne le nombre de hachages Argon2 simultanés (mémoire/CPU).
    pub hash_slots: Semaphore,
    pub dummy_hash: String,
    pub ceremonies: Mutex<HashMap<String, (Ceremony, Instant)>>,
    pub git: Arc<GitStore>,
    pub pages: Arc<PageCache>,
}

impl AppState {
    pub fn put_ceremony(&self, key: String, c: Ceremony) {
        let mut map = self.ceremonies.lock().unwrap();
        map.retain(|_, (_, t)| t.elapsed() < Duration::from_secs(300));
        map.insert(key, (c, Instant::now()));
    }

    pub fn take_ceremony(&self, key: &str) -> Option<Ceremony> {
        let (c, t) = self.ceremonies.lock().unwrap().remove(key)?;
        (t.elapsed() < Duration::from_secs(300)).then_some(c)
    }
}
