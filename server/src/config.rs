use std::{env, net::SocketAddr, path::PathBuf};

#[derive(Clone, Debug)]
pub struct Config {
    pub bind: SocketAddr,
    pub database_url: String,
    pub data_dir: PathBuf,
    /// URL publique du site (ex. https://justprivacy.example). Sert d'origine WebAuthn,
    /// de contrôle CSRF et décide du drapeau Secure des cookies.
    pub public_url: url::Url,
    pub cap_siteverify_url: String,
    pub cap_secret: Option<String>,
    pub cap_disabled: bool,
    /// Dossier contenant build.py, templates/, fonts/ et content/.
    pub site_src: PathBuf,
    pub python: String,
    pub trust_proxy_hops: usize,
    pub app_secret: Option<String>,
}

fn var(name: &str) -> Option<String> {
    env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

impl Config {
    pub fn from_env() -> Result<Config, String> {
        let public_url = var("PUBLIC_URL").unwrap_or_else(|| "http://localhost:8080".into());
        let public_url = url::Url::parse(&public_url).map_err(|e| format!("PUBLIC_URL invalide : {e}"))?;
        if public_url.host_str().is_none() {
            return Err("PUBLIC_URL doit contenir un nom d'hôte".into());
        }
        let cap_disabled = matches!(var("CAP_DISABLED").as_deref(), Some("1" | "true" | "yes"));
        let cap_secret = var("CAP_SECRET");
        if cap_secret.is_none() && !cap_disabled {
            return Err("CAP_SECRET manquant (ou CAP_DISABLED=true pour le développement uniquement)".into());
        }
        Ok(Config {
            bind: var("BIND").unwrap_or_else(|| "0.0.0.0:8080".into()).parse().map_err(|e| format!("BIND invalide : {e}"))?,
            database_url: var("DATABASE_URL").ok_or("DATABASE_URL manquant")?,
            data_dir: PathBuf::from(var("DATA_DIR").unwrap_or_else(|| "./data".into())),
            public_url,
            cap_siteverify_url: var("CAP_SITEVERIFY_URL")
                .unwrap_or_else(|| "https://check.scrawlee.com/5c7f97279d/siteverify".into()),
            cap_secret,
            cap_disabled,
            site_src: PathBuf::from(var("SITE_SRC").unwrap_or_else(|| "..".into())),
            python: var("PYTHON").unwrap_or_else(|| "python3".into()),
            trust_proxy_hops: var("TRUST_PROXY_HOPS").and_then(|v| v.parse().ok()).unwrap_or(1),
            app_secret: var("APP_SECRET"),
        })
    }

    pub fn secure_cookies(&self) -> bool {
        self.public_url.scheme() == "https"
    }

    pub fn session_cookie(&self) -> &'static str {
        if self.secure_cookies() { "__Host-jp_session" } else { "jp_session" }
    }
}
