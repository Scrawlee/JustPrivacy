//! Pages du site, rendues à la demande depuis git et gardées en mémoire.
//!
//! - Une page est rendue au premier accès (gabarits embarqués + articles de la branche `main`), puis servie depuis le cache.
//! - Chaque page porte des étiquettes de dépendance (`article:<slug>`, `articles`). Quand `main` change — modification par
//!   l'éditeur web, push git, suppression — on compare les articles de l'ancien et du nouveau commit et on ne purge que
//!   les pages concernées : l'article modifié, la liste et l'accueil, et les articles qui le citent en « voir aussi ».
//! - Les pages sans article (benchmark, à propos, comptes, éditeur…) restent en cache tant que le serveur tourne.
use crate::{gitrepo::{GitResult, GitStore}, render};
use axum::body::Bytes;
use git2::Oid;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    sync::{atomic::{AtomicU64, Ordering}, Arc, Mutex, RwLock},
    time::Instant,
};

/// Page prête à être servie.
pub struct Cached {
    pub body: Bytes,
    pub etag: String,
    tags: Vec<String>,
}

#[derive(Default)]
struct Inner {
    head: Option<Oid>,
    blobs: BTreeMap<String, String>,
    set: Option<Arc<render::ArticleSet>>,
    pages: HashMap<String, Arc<Cached>>,
}

#[derive(Default, serde::Serialize, Clone)]
pub struct Stats {
    pub renders: u64,
    pub purges: u64,
    pub cached_pages: usize,
}

pub struct PageCache {
    inner: RwLock<Inner>,
    started: Instant,
    last_check_ms: AtomicU64,
    stats: Mutex<Stats>,
}

/// Une page qui dépend des articles ?
fn needs_articles(path: &str) -> bool {
    path == "/" || path == "/articles" || path.starts_with("/articles/")
}

impl PageCache {
    pub fn new() -> Arc<PageCache> {
        Arc::new(PageCache { inner: RwLock::default(), started: Instant::now(), last_check_ms: AtomicU64::new(0), stats: Mutex::default() })
    }

    pub fn stats(&self) -> Stats {
        let mut s = self.stats.lock().unwrap().clone();
        s.cached_pages = self.inner.read().unwrap().pages.len();
        s
    }

    /// Compare `main` au dernier état connu et purge les pages dont un article a changé. Renvoie les articles touchés.
    pub fn sync(&self, git: &GitStore) -> GitResult<Vec<String>> {
        self.last_check_ms.store(self.started.elapsed().as_millis() as u64, Ordering::Relaxed);
        let head = git.head_oid()?;
        if self.inner.read().unwrap().head == Some(head) { return Ok(vec![]); }
        let blobs = git.article_blobs(head)?;
        let mut inner = self.inner.write().unwrap();
        if inner.head == Some(head) { return Ok(vec![]); }
        let mut changed: Vec<String> = vec![];
        if inner.head.is_some() {
            for (slug, blob) in &blobs {
                if inner.blobs.get(slug) != Some(blob) { changed.push(slug.clone()); }
            }
            for slug in inner.blobs.keys() {
                if !blobs.contains_key(slug) { changed.push(slug.clone()); }
            }
        }
        if !changed.is_empty() {
            let mut stale: Vec<String> = changed.iter().map(|s| format!("article:{s}")).collect();
            stale.push("articles".into());
            let before = inner.pages.len();
            inner.pages.retain(|_, p| !p.tags.iter().any(|t| stale.contains(t)));
            self.stats.lock().unwrap().purges += (before - inner.pages.len()) as u64;
            tracing::info!(articles = ?changed, purged = before - inner.pages.len(), "pages purgées");
        }
        inner.head = Some(head);
        inner.blobs = blobs;
        inner.set = None;
        Ok(changed)
    }

    /// Contrôle (au plus toutes les 500 ms) que `main` n'a pas bougé hors de nos écritures, par exemple après un `git push`.
    fn sync_if_due(&self, git: &GitStore) -> GitResult<()> {
        let now = self.started.elapsed().as_millis() as u64;
        if now.saturating_sub(self.last_check_ms.load(Ordering::Relaxed)) >= 500 || self.inner.read().unwrap().head.is_none() {
            self.sync(git)?;
        }
        Ok(())
    }

    fn article_set(&self, git: &GitStore) -> GitResult<Arc<render::ArticleSet>> {
        if let Some(set) = self.inner.read().unwrap().set.clone() { return Ok(set); }
        let (head, raw) = git.load_articles()?;
        let set = Arc::new(render::ArticleSet::new(&raw));
        let mut inner = self.inner.write().unwrap();
        // Ne garde l'ensemble que s'il correspond encore à l'état suivi (sinon la prochaine synchronisation le recharge).
        if inner.head == Some(head) { inner.set = Some(set.clone()); }
        Ok(set)
    }

    /// Page pour un chemin normalisé ; `None` : 404. Les 404 ne sont pas gardées en cache.
    pub fn get(&self, git: &GitStore, path: &str) -> GitResult<Option<Arc<Cached>>> {
        if needs_articles(path) { self.sync_if_due(git)?; }
        if let Some(p) = self.inner.read().unwrap().pages.get(path) { return Ok(Some(p.clone())); }
        let set = if needs_articles(path) { self.article_set(git)? } else { Arc::new(render::ArticleSet { list: vec![] }) };
        let epoch = self.inner.read().unwrap().head;
        let Some(r) = render::render_route(path, &set) else { return Ok(None) };
        let mut hash = Sha256::new();
        hash.update(r.html.as_bytes());
        let etag = format!("\"{}\"", hex::encode(&hash.finalize()[..12]));
        let page = Arc::new(Cached { body: Bytes::from(r.html.into_bytes()), etag, tags: r.tags });
        self.stats.lock().unwrap().renders += 1;
        let mut inner = self.inner.write().unwrap();
        // Une purge a pu avoir lieu pendant le rendu : on ne mémorise que si `main` n'a pas bougé entre-temps.
        if !needs_articles(path) || inner.head == epoch {
            inner.pages.insert(path.to_string(), page.clone());
        }
        Ok(Some(page))
    }

    pub fn head(&self) -> Option<String> {
        self.inner.read().unwrap().head.map(|h| h.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf};

    fn art(slug: &str, title: &str, see: &str) -> String {
        format!("---\norder: 1\nslug: {slug}\nlang: fr\ntitle_fr: {title}\nsee_also: {see}\n---\n\nTexte de {slug}.\n")
    }

    fn store() -> (GitStore, PathBuf) {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("jp-pages-test-{}-{}-{n}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let seed = dir.join("seed");
        fs::create_dir_all(&seed).unwrap();
        for (slug, see) in [("a", "b"), ("b", ""), ("c", "")] {
            fs::write(seed.join(format!("{slug}.md")), art(slug, &slug.to_uppercase(), see)).unwrap();
        }
        (GitStore::open_or_init(&dir, &seed).unwrap(), dir)
    }

    fn warm(cache: &PageCache, git: &GitStore) {
        for p in ["/", "/articles", "/articles/a", "/articles/b", "/articles/c", "/benchmark"] {
            assert!(cache.get(git, p).unwrap().is_some(), "{p}");
        }
    }

    fn renders(cache: &PageCache) -> u64 { cache.stats().renders }

    #[test]
    fn serves_from_memory_after_the_first_render() {
        let (git, dir) = store();
        let cache = PageCache::new();
        warm(&cache, &git);
        let first = renders(&cache);
        assert_eq!(first, 6);
        warm(&cache, &git);
        assert_eq!(renders(&cache), first, "deuxième passage : tout vient du cache");
        let p1 = cache.get(&git, "/articles/a").unwrap().unwrap();
        let p2 = cache.get(&git, "/articles/a").unwrap().unwrap();
        assert_eq!(p1.etag, p2.etag);
        assert!(cache.get(&git, "/articles/inconnu").unwrap().is_none());
        assert!(cache.get(&git, "/nimporte/quoi").unwrap().is_none());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_change_purges_only_the_pages_that_depend_on_it() {
        let (git, dir) = store();
        let cache = PageCache::new();
        warm(&cache, &git);
        let before = renders(&cache);
        let etag_c = cache.get(&git, "/articles/c").unwrap().unwrap().etag.clone();

        // Modification de « c » : seuls c, la liste et l'accueil sont rendus de nouveau.
        let (_, v) = git.read_article("c", None).unwrap();
        git.write_article("c", &art("c", "C modifié", ""), Some(&v), "alice", "").unwrap();
        assert_eq!(cache.sync(&git).unwrap(), vec!["c"]);
        warm(&cache, &git);
        assert_eq!(renders(&cache) - before, 3, "c + liste + accueil");
        assert_ne!(cache.get(&git, "/articles/c").unwrap().unwrap().etag, etag_c);
        assert!(String::from_utf8_lossy(&cache.get(&git, "/articles/c").unwrap().unwrap().body).contains("C modifié"));

        // Modification de « b », cité en « voir aussi » par « a » : b, a, la liste et l'accueil.
        let before = renders(&cache);
        let (_, v) = git.read_article("b", None).unwrap();
        git.write_article("b", &art("b", "B modifié", ""), Some(&v), "alice", "").unwrap();
        assert_eq!(cache.sync(&git).unwrap(), vec!["b"]);
        warm(&cache, &git);
        assert_eq!(renders(&cache) - before, 4, "b + a + liste + accueil");
        assert!(String::from_utf8_lossy(&cache.get(&git, "/articles/a").unwrap().unwrap().body).contains("B modifié"));

        // Une page sans article n'est jamais purgée.
        assert_eq!(cache.stats().cached_pages, 6);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn deletions_and_creations_are_noticed_even_without_an_explicit_sync() {
        let (git, dir) = store();
        let cache = PageCache::new();
        warm(&cache, &git);
        let (_, v) = git.read_article("c", None).unwrap();
        git.delete_article("c", &v, "alice").unwrap();
        // Pas d'appel à sync() : la vérification périodique de `main` (par ex. après un git push) s'en charge.
        std::thread::sleep(std::time::Duration::from_millis(600));
        assert!(cache.get(&git, "/articles/c").unwrap().is_none(), "article supprimé : 404");
        git.write_article("d", &art("d", "D", ""), None, "alice", "").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(600));
        assert!(cache.get(&git, "/articles/d").unwrap().is_some());
        assert!(String::from_utf8_lossy(&cache.get(&git, "/articles").unwrap().unwrap().body).contains("/articles/d"));
        let _ = fs::remove_dir_all(dir);
    }
}
