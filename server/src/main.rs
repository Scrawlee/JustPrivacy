mod article;
mod auth;
mod captcha;
mod config;
mod crypto;
mod error;
mod githttp;
mod gitrepo;
mod mfa;
mod ratelimit;
mod routes_account;
mod routes_articles;
mod routes_auth;
mod pages;
mod render;
mod state;

use axum::{
    extract::State,
    http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode, Uri},
    middleware,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use config::Config;
use sqlx::postgres::PgPoolOptions;
use state::AppState;
use std::{collections::HashMap, net::SocketAddr, sync::{Arc, Mutex}, time::Duration};
use tower_http::{compression::CompressionLayer, set_header::SetResponseHeaderLayer};
use webauthn_rs::WebauthnBuilder;

const CSP: &str = "default-src 'self'; script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval' https://check.scrawlee.com; \
style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self' data:; \
connect-src 'self' https://check.scrawlee.com; worker-src blob:; \
frame-ancestors 'none'; base-uri 'none'; form-action 'self'";

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .without_time()
        .init();
    if let Err(e) = run().await {
        eprintln!("erreur fatale : {e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let mut cfg = Config::from_env().or_else(|e| {
        // La sous-commande « promote » n'a pas besoin de Cap.
        if std::env::args().nth(1).as_deref() == Some("promote") && std::env::var("DATABASE_URL").is_ok() {
            std::env::set_var("CAP_DISABLED", "true");
            Config::from_env()
        } else { Err(e) }
    })?;
    std::fs::create_dir_all(&cfg.data_dir).map_err(|e| e.to_string())?;
    cfg.data_dir = cfg.data_dir.canonicalize().map_err(|e| e.to_string())?;

    let db = PgPoolOptions::new()
        .max_connections(10)
        .acquire_timeout(Duration::from_secs(10))
        .connect(&cfg.database_url)
        .await
        .map_err(|e| format!("connexion PostgreSQL : {e}"))?;
    sqlx::migrate!("./migrations").run(&db).await.map_err(|e| format!("migrations : {e}"))?;

    // « justprivacy-server promote <utilisateur> <user|editor|admin> »
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("promote") {
        let (Some(user), Some(role)) = (args.get(2), args.get(3)) else {
            return Err("usage : justprivacy-server promote <utilisateur> <user|editor|admin>".into());
        };
        if !["user", "editor", "admin"].contains(&role.as_str()) { return Err("rôle inconnu".into()); }
        let n = sqlx::query("UPDATE users SET role = $2 WHERE username = $1")
            .bind(user.to_lowercase()).bind(role).execute(&db).await.map_err(|e| e.to_string())?.rows_affected();
        if n == 0 { return Err("utilisateur introuvable".into()); }
        println!("{user} est maintenant {role}");
        return Ok(());
    }

    let origin = cfg.public_url.clone();
    let webauthn = WebauthnBuilder::new(origin.host_str().unwrap_or("localhost"), &origin)
        .map_err(|e| format!("WebAuthn : {e}"))?
        .rp_name("JustPrivacy")
        .build()
        .map_err(|e| format!("WebAuthn : {e}"))?;
    let sealer = crypto::Sealer::load(cfg.app_secret.as_deref(), &cfg.data_dir).map_err(|e| e.to_string())?;
    let git = Arc::new(
        gitrepo::GitStore::open_or_init(&cfg.data_dir, &cfg.site_src.join("content").join("articles")).map_err(|e| format!("git : {e:?}"))?,
    );
    let pages = pages::PageCache::new();
    // Premier état de `main` : les pages sont ensuite rendues à la demande.
    { let (p, g) = (pages.clone(), git.clone()); let _ = tokio::task::spawn_blocking(move || p.sync(&g)).await; }

    let st = Arc::new(AppState {
        db,
        webauthn,
        sealer,
        http: reqwest::Client::builder().timeout(Duration::from_secs(8)).build().map_err(|e| e.to_string())?,
        limiter: Default::default(),
        hash_slots: tokio::sync::Semaphore::new(4),
        dummy_hash: auth::new_hasher_hash("mot de passe factice"),
        ceremonies: Mutex::new(HashMap::new()),
        git,
        pages,
        cfg: cfg.clone(),
    });

    let api = Router::new()
        .merge(routes_auth::router())
        .merge(routes_account::router())
        .merge(routes_articles::router())
        .layer(middleware::from_fn_with_state(st.clone(), auth::csrf_guard))
        .layer(SetResponseHeaderLayer::overriding(header::CACHE_CONTROL, HeaderValue::from_static("no-store")));

    let app = Router::new()
        .nest("/api", api)
        .merge(githttp::router())
        .route("/healthz", get(|| async { "ok" }))
        .route("/favicon.svg", get(|| async { icon("image/svg+xml", include_bytes!("../../templates/favicon.svg")) }))
        .route("/favicon.ico", get(|| async { icon("image/x-icon", include_bytes!("../../templates/favicon.ico")) }))
        .route("/apple-touch-icon.png", get(|| async { icon("image/png", include_bytes!("../../templates/apple-touch-icon.png")) }))
        .fallback(get(site_page).layer(CompressionLayer::new()))
        .layer(SetResponseHeaderLayer::if_not_present(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")))
        .layer(SetResponseHeaderLayer::if_not_present(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY")))
        .layer(SetResponseHeaderLayer::if_not_present(header::REFERRER_POLICY, HeaderValue::from_static("same-origin")))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("permissions-policy"),
            HeaderValue::from_static("camera=(), microphone=(), geolocation=(), publickey-credentials-get=(self), publickey-credentials-create=(self)"),
        ))
        .with_state(st);

    let listener = tokio::net::TcpListener::bind(cfg.bind).await.map_err(|e| format!("bind {} : {e}", cfg.bind))?;
    tracing::info!("à l'écoute sur {}", cfg.bind);
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(async { let _ = tokio::signal::ctrl_c().await; })
        .await
        .map_err(|e| e.to_string())
}

/// Site icons: embedded in the binary, cached for a day.
fn icon(content_type: &'static str, body: &'static [u8]) -> Response {
    let mut r = body.into_response();
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=86400"));
    r
}

/// Chemin demandé → chemin de page normalisé (sans barre finale) ; `None` : chemin refusé.
fn page_path(raw: &str) -> Option<String> {
    let unsafe_chars = raw.contains('%') || raw.contains('\\') || raw.contains('\0');
    if unsafe_chars || raw.split('/').any(|s| s == ".." || s == ".") { return None; }
    let p = raw.trim_end_matches('/');
    Some(match p { "" | "/index.html" => "/".to_string(), other => other.to_string() })
}

fn not_found() -> Response {
    let mut r = (StatusCode::NOT_FOUND, render::not_found_page()).into_response();
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(CSP));
    r
}

/// Sert les pages du site : rendues depuis git à la première demande, puis gardées en mémoire (voir `pages.rs`).
async fn site_page(State(st): State<Arc<AppState>>, headers: HeaderMap, uri: Uri) -> Response {
    let Some(path) = page_path(uri.path()) else { return not_found() };
    let (pages, git) = (st.pages.clone(), st.git.clone());
    let page = match tokio::task::spawn_blocking(move || pages.get(&git, &path)).await {
        Ok(Ok(Some(p))) => p,
        Ok(Ok(None)) => return not_found(),
        Ok(Err(e)) => { tracing::error!("rendu de page : {e:?}"); return (StatusCode::INTERNAL_SERVER_ERROR, "Erreur interne").into_response(); }
        Err(e) => { tracing::error!("rendu de page : {e}"); return (StatusCode::INTERNAL_SERVER_ERROR, "Erreur interne").into_response(); }
    };
    let unchanged = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').map(str::trim).any(|t| t == page.etag || t.strip_prefix("W/") == Some(page.etag.as_str())));
    let mut r = if unchanged { StatusCode::NOT_MODIFIED.into_response() } else { (StatusCode::OK, page.body.clone()).into_response() };
    let h = r.headers_mut();
    if !unchanged { h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8")); }
    if let Ok(etag) = HeaderValue::from_str(&page.etag) { h.insert(header::ETAG, etag); }
    // Toujours revalider (ETag) : le serveur purge précisément ses pages quand un article change.
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=0, must-revalidate"));
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(CSP));
    r
}
