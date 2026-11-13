//! Construction du site statique à partir des articles du dépôt git (build.py), avec bascule atomique.
use crate::{config::Config, gitrepo::GitStore};
use std::{fs, path::{Path, PathBuf}, sync::{Arc, Mutex}, time::Duration};
use tokio::{process::Command, sync::mpsc};

#[derive(Clone, Default, serde::Serialize)]
pub struct Status {
    pub ok: bool,
    pub commit: String,
    pub error: Option<String>,
}

pub struct Site {
    tx: mpsc::Sender<()>,
    pub status: Mutex<Status>,
}

impl Site {
    pub fn new() -> (Arc<Site>, mpsc::Receiver<()>) {
        let (tx, rx) = mpsc::channel(1);
        (Arc::new(Site { tx, status: Mutex::new(Status::default()) }), rx)
    }

    /// Demande une reconstruction (les demandes rapprochées sont regroupées).
    pub fn trigger(&self) {
        let _ = self.tx.try_send(());
    }
}

pub fn current_dir(cfg: &Config) -> PathBuf {
    cfg.data_dir.join("site").join("current")
}

async fn build_once(git: &Arc<GitStore>, cfg: &Config) -> Result<String, String> {
    let builds = cfg.data_dir.join("site").join("builds");
    let g = git.clone();
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let work = builds.join(stamp.to_string());
    let content = work.join("content");
    let w = content.clone();
    let commit = tokio::task::spawn_blocking(move || g.export_articles(&w))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:?}"))?;
    let out = work.join("dist");
    let run = Command::new(&cfg.python)
        .arg("build.py")
        .arg("--content").arg(&content)
        .arg("--out").arg(&out)
        .current_dir(&cfg.site_src)
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(Duration::from_secs(120), run)
        .await
        .map_err(|_| "build.py : délai dépassé".to_string())?
        .map_err(|e| format!("build.py : {e}"))?;
    if !output.status.success() || !out.join("index.html").exists() {
        let _ = fs::remove_dir_all(&work);
        return Err(String::from_utf8_lossy(&output.stderr).trim().chars().take(2000).collect());
    }
    // Bascule atomique : le lien symbolique « current » est remplacé d'un seul rename().
    let site = cfg.data_dir.join("site");
    let tmp_link = site.join("current.new");
    let _ = fs::remove_file(&tmp_link);
    #[cfg(unix)]
    std::os::unix::fs::symlink(&out, &tmp_link).map_err(|e| e.to_string())?;
    fs::rename(&tmp_link, site.join("current")).map_err(|e| e.to_string())?;
    prune(&builds, &work);
    Ok(commit.to_string())
}

/// Ne garde que la construction courante et la précédente.
fn prune(builds: &Path, keep: &Path) {
    let mut dirs: Vec<PathBuf> = fs::read_dir(builds).into_iter().flatten().flatten().map(|e| e.path()).collect();
    dirs.sort();
    let n = dirs.len();
    for d in dirs.into_iter().take(n.saturating_sub(2)) {
        if d != keep { let _ = fs::remove_dir_all(d); }
    }
}

/// Reconstruit le site une fois au démarrage, puis à chaque demande.
pub async fn worker(site: Arc<Site>, mut rx: mpsc::Receiver<()>, git: Arc<GitStore>, cfg: Config) {
    loop {
        let result = build_once(&git, &cfg).await;
        {
            let mut s = site.status.lock().unwrap();
            match result {
                Ok(commit) => {
                    tracing::info!("site reconstruit ({commit})");
                    *s = Status { ok: true, commit, error: None };
                }
                Err(e) => {
                    tracing::error!("échec de la construction du site, l'ancienne version reste en ligne : {e}");
                    s.ok = false;
                    s.error = Some(e);
                }
            }
        }
        if rx.recv().await.is_none() { break; }
        tokio::time::sleep(Duration::from_millis(500)).await;
        while rx.try_recv().is_ok() {}
    }
}
