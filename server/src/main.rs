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
mod site;
mod state;

use axum::{
    extract::{ConnectInfo, State},
    http::{header, HeaderName, HeaderValue, StatusCode, Uri},
    middleware,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use config::Config;
use sqlx::postgres::PgPoolOptions;
use state::AppState;
use std::{collections::HashMap, net::SocketAddr, path::PathBuf, sync::{Arc, Mutex}, time::Duration};
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
    let (site, rx) = site::Site::new();
    tokio::spawn(site::worker(site.clone(), rx, git.clone(), cfg.clone()));

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
        site,
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
        .fallback(get(static_files).layer(CompressionLayer::new()))
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

/// Sert le site construit : `/x` → `x`, `x/index.html` ou `x.html` ; 404.html sinon.
async fn static_files(State(st): State<Arc<AppState>>, _peer: ConnectInfo<SocketAddr>, uri: Uri) -> Response {
    let root = site::current_dir(&st.cfg);
    let path = uri.path().trim_matches('/');
    let safe = !path.contains('%') && !path.contains('\\') && !path.contains('\0')
        && path.split('/').all(|s| s != ".." && s != ".");
    if safe {
        let base: PathBuf = root.join(path);
        let candidates = [base.clone(), base.join("index.html"), PathBuf::from(format!("{}.html", base.display()))];
        for c in candidates {
            if c.is_file() && c.starts_with(&root) {
                if let Ok(bytes) = tokio::fs::read(&c).await {
                    return file_response(&c, bytes, StatusCode::OK);
                }
            }
        }
    }
    let nf = root.join("404.html");
    match tokio::fs::read(&nf).await {
        Ok(bytes) => file_response(&nf, bytes, StatusCode::NOT_FOUND),
        Err(_) => (StatusCode::NOT_FOUND, "Page introuvable").into_response(),
    }
}

fn file_response(path: &std::path::Path, bytes: Vec<u8>, status: StatusCode) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let mut r = (status, bytes).into_response();
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_str(&format!("{mime}; charset=utf-8")).unwrap_or(HeaderValue::from_static("text/html; charset=utf-8")));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=0, must-revalidate"));
    if mime.essence_str() == "text/html" {
        h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(CSP));
    }
    r
}
