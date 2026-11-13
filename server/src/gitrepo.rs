//! Dépôt git des articles (bare, géré par libgit2) : lecture, historique, écriture, export.
use crate::article;
use git2::{Commit, ErrorCode, ObjectType, Oid, Repository, RepositoryInitOptions, Signature, Sort, Tree};
use serde::Serialize;
use std::{fs, path::{Path, PathBuf}, sync::Mutex};

pub const MAIN_REF: &str = "refs/heads/main";
pub const ARTICLES_DIR: [&str; 2] = ["content", "articles"];
pub const MAX_ARTICLE_BYTES: usize = 200_000;

#[derive(Debug)]
pub enum GitError {
    NotFound,
    /// L'article a changé depuis la version que l'éditeur avait ouverte.
    Conflict,
    Invalid(&'static str),
    Other(String),
}

impl From<git2::Error> for GitError {
    fn from(e: git2::Error) -> Self { GitError::Other(e.message().to_string()) }
}
impl From<std::io::Error> for GitError {
    fn from(e: std::io::Error) -> Self { GitError::Other(e.to_string()) }
}

pub type GitResult<T> = Result<T, GitError>;

#[derive(Serialize)]
pub struct Revision {
    pub commit: String,
    pub author: String,
    pub time: i64,
    pub message: String,
}

/// Article de la branche principale, avec les champs utiles à l'éditeur.
#[derive(Serialize)]
pub struct ArticleInfo {
    pub slug: String,
    pub version: String,
    pub order: u32,
    pub lang: String,
    pub title_fr: String,
    pub title_en: String,
}

/// Proposition : version d'un article poussée sur `users/<nom>/<slug>`, en attente de relecture.
#[derive(Serialize)]
pub struct Proposal {
    pub user: String,
    pub slug: String,
    pub commit: String,
    pub time: i64,
    pub message: String,
    pub title: String,
    /// L'article n'existe pas encore sur la branche principale.
    pub is_new: bool,
}

/// Nombre maximal de propositions ouvertes par compte.
pub const MAX_PROPOSALS_PER_USER: usize = 30;

pub struct GitStore {
    path: PathBuf,
    /// Sérialise les écritures (commits web et pushes).
    pub write_lock: Mutex<()>,
}

pub fn valid_slug(slug: &str) -> bool {
    !slug.is_empty() && slug.len() <= 64 && slug.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !slug.starts_with('-')
}

fn article_path(slug: &str) -> String {
    format!("{}/{}/{}.md", ARTICLES_DIR[0], ARTICLES_DIR[1], slug)
}

fn bot() -> Signature<'static> {
    Signature::now("JustPrivacy", "noreply@justprivacy.invalid").expect("signature")
}

impl GitStore {
    /// Ouvre le dépôt, ou le crée avec les articles de `seed_dir` comme premier commit.
    pub fn open_or_init(data_dir: &Path, seed_dir: &Path) -> GitResult<GitStore> {
        let path = data_dir.join("git").join("articles.git");
        let store = GitStore { path: path.clone(), write_lock: Mutex::new(()) };
        if !path.exists() {
            fs::create_dir_all(&path)?;
            let mut opts = RepositoryInitOptions::new();
            opts.bare(true).initial_head("main");
            let repo = Repository::init_opts(&path, &opts)?;
            store.seed(&repo, seed_dir)?;
        }
        let repo = store.repo()?;
        if repo.find_reference(MAIN_REF).is_err() {
            store.seed(&repo, seed_dir)?;
        }
        Ok(store)
    }

    fn seed(&self, repo: &Repository, seed_dir: &Path) -> GitResult<()> {
        let mut tb = repo.treebuilder(None)?;
        if let Ok(rd) = fs::read_dir(seed_dir) {
            let mut files: Vec<_> = rd.filter_map(Result::ok).filter(|e| e.path().extension().is_some_and(|x| x == "md")).collect();
            files.sort_by_key(|e| e.file_name());
            for f in files {
                let blob = repo.blob(&fs::read(f.path())?)?;
                tb.insert(f.file_name().to_string_lossy().as_ref(), blob, 0o100644)?;
            }
        }
        let articles = tb.write()?;
        let mut content = repo.treebuilder(None)?;
        content.insert(ARTICLES_DIR[1], articles, 0o040000)?;
        let mut root = repo.treebuilder(None)?;
        root.insert(ARTICLES_DIR[0], content.write()?, 0o040000)?;
        let tree = repo.find_tree(root.write()?)?;
        let sig = bot();
        repo.commit(Some(MAIN_REF), &sig, &sig, "Import initial des articles", &tree, &[])?;
        Ok(())
    }

    pub fn repo(&self) -> GitResult<Repository> {
        Ok(Repository::open_bare(&self.path)?)
    }

    pub fn head_oid(&self) -> GitResult<Oid> {
        Ok(self.repo()?.refname_to_id(MAIN_REF)?)
    }

    fn articles_tree<'r>(&self, repo: &'r Repository, commit: Oid) -> GitResult<Option<Tree<'r>>> {
        let tree = repo.find_commit(commit)?.tree()?;
        match tree.get_path(Path::new(&format!("{}/{}", ARTICLES_DIR[0], ARTICLES_DIR[1]))) {
            Ok(e) => Ok(Some(repo.find_tree(e.id())?)),
            Err(e) if e.code() == ErrorCode::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Articles de la branche principale, triés par ordre puis par nom.
    pub fn list_articles(&self) -> GitResult<Vec<ArticleInfo>> {
        let repo = self.repo()?;
        let Some(tree) = self.articles_tree(&repo, self.head_oid()?)? else { return Ok(vec![]) };
        let mut out = vec![];
        for e in tree.iter().filter(|e| e.kind() == Some(ObjectType::Blob)) {
            let Some(slug) = e.name().and_then(|n| n.strip_suffix(".md")) else { continue };
            let blob = repo.find_blob(e.id())?;
            let text = String::from_utf8_lossy(blob.content()).into_owned();
            let meta = article::parse(&text).map(|(m, _)| m).unwrap_or_default();
            out.push(ArticleInfo {
                slug: slug.to_string(),
                version: e.id().to_string(),
                order: meta.get("order").and_then(|o| o.parse().ok()).unwrap_or(999),
                lang: article::lang_of(&meta).to_string(),
                title_fr: meta.get("title_fr").cloned().unwrap_or_default(),
                title_en: meta.get("title_en").cloned().unwrap_or_default(),
            });
        }
        out.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.slug.cmp(&b.slug)));
        Ok(out)
    }

    fn blob_at(&self, repo: &Repository, commit: Oid, slug: &str) -> GitResult<Option<Oid>> {
        match repo.find_commit(commit)?.tree()?.get_path(Path::new(&article_path(slug))) {
            Ok(e) if e.kind() == Some(ObjectType::Blob) => Ok(Some(e.id())),
            Ok(_) => Ok(None),
            Err(e) if e.code() == ErrorCode::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn read_article(&self, slug: &str, at: Option<Oid>) -> GitResult<(String, String)> {
        if !valid_slug(slug) { return Err(GitError::NotFound); }
        let repo = self.repo()?;
        let commit = match at { Some(c) => c, None => self.head_oid()? };
        let blob = self.blob_at(&repo, commit, slug)?.ok_or(GitError::NotFound)?;
        let content = String::from_utf8_lossy(repo.find_blob(blob)?.content()).into_owned();
        Ok((content, blob.to_string()))
    }

    /// Commits qui ont modifié l'article, du plus récent au plus ancien.
    pub fn history(&self, slug: &str, limit: usize) -> GitResult<Vec<Revision>> {
        if !valid_slug(slug) { return Err(GitError::NotFound); }
        let repo = self.repo()?;
        let mut walk = repo.revwalk()?;
        walk.set_sorting(Sort::TIME)?;
        walk.push(self.head_oid()?)?;
        let mut out = vec![];
        for oid in walk.take(2000) {
            let commit = repo.find_commit(oid?)?;
            let here = self.blob_at(&repo, commit.id(), slug)?;
            let before = match commit.parent_id(0) {
                Ok(p) => self.blob_at(&repo, p, slug)?,
                Err(_) => None,
            };
            if here != before {
                out.push(Revision {
                    commit: commit.id().to_string(),
                    author: commit.author().name().unwrap_or("?").to_string(),
                    time: commit.time().seconds(),
                    message: commit.message().unwrap_or("").trim().to_string(),
                });
                if out.len() >= limit { break; }
            }
        }
        Ok(out)
    }

    /// Identifiants de blob des articles à un commit donné (slug → blob), sans lire les contenus.
    pub fn article_blobs(&self, commit: Oid) -> GitResult<std::collections::BTreeMap<String, String>> {
        let repo = self.repo()?;
        let mut out = std::collections::BTreeMap::new();
        if let Some(tree) = self.articles_tree(&repo, commit)? {
            for e in tree.iter().filter(|e| e.kind() == Some(ObjectType::Blob)) {
                if let Some(slug) = e.name().and_then(|n| n.strip_suffix(".md")) {
                    out.insert(slug.to_string(), e.id().to_string());
                }
            }
        }
        Ok(out)
    }

    /// Contenu de tous les articles de la branche principale : (commit, [(slug, texte)]), par ordre alphabétique.
    pub fn load_articles(&self) -> GitResult<(Oid, Vec<(String, String)>)> {
        let repo = self.repo()?;
        let head = self.head_oid()?;
        let mut out = vec![];
        if let Some(tree) = self.articles_tree(&repo, head)? {
            for e in tree.iter().filter(|e| e.kind() == Some(ObjectType::Blob)) {
                let Some(slug) = e.name().and_then(|n| n.strip_suffix(".md")) else { continue };
                if !valid_slug(slug) { continue; }
                out.push((slug.to_string(), String::from_utf8_lossy(repo.find_blob(e.id())?.content()).into_owned()));
            }
        }
        Ok((head, out))
    }

    /// Valide un article puis l'enregistre sous forme de blob.
    fn checked_blob(&self, repo: &Repository, slug: &str, content: &str) -> GitResult<Oid> {
        if !valid_slug(slug) { return Err(GitError::Invalid("invalid_slug")); }
        if content.len() > MAX_ARTICLE_BYTES { return Err(GitError::Invalid("too_large")); }
        article::validate(slug, content).map_err(GitError::Invalid)?;
        Ok(repo.blob(content.as_bytes())?)
    }

    /// Crée un commit qui ajoute, modifie (`blob`) ou supprime (`None`) l'article, sur `refname`.
    #[allow(clippy::too_many_arguments)]
    fn commit_article(repo: &Repository, parent: &Commit, refname: &str, slug: &str, blob: Option<Oid>,
                      author: &Signature, committer: &Signature, message: &str) -> GitResult<Oid> {
        let path = [ARTICLES_DIR[0], ARTICLES_DIR[1], &format!("{slug}.md")];
        let base = parent.tree()?;
        let new_tree = match blob {
            Some(b) => update_tree(repo, Some(&base), &path, b)?,
            None => remove_from_tree(repo, &base, &path)?,
        };
        let tree = repo.find_tree(new_tree)?;
        if tree.id() == parent.tree_id() { return Ok(parent.id()); }
        Ok(repo.commit(Some(refname), author, committer, message, &tree, &[parent])?)
    }

    fn user_sig(user: &str) -> GitResult<Signature<'static>> {
        Ok(Signature::now(user, &format!("{user}@users.noreply.justprivacy.invalid"))?)
    }

    fn clean_message(message: &str, default: String) -> String {
        if message.trim().is_empty() { default } else { message.trim().chars().take(500).collect() }
    }

    /// Crée ou modifie un article sur la branche principale. `base_blob` est la version que l'auteur
    /// a lue (None pour une création). Renvoie (commit, version du blob écrit).
    pub fn write_article(&self, slug: &str, content: &str, base_blob: Option<&str>, author: &str, message: &str) -> GitResult<(String, String)> {
        let _guard = self.write_lock.lock().unwrap();
        let repo = self.repo()?;
        let blob = self.checked_blob(&repo, slug, content)?;
        let head = repo.find_commit(self.head_oid()?)?;
        let current = self.blob_at(&repo, head.id(), slug)?.map(|o| o.to_string());
        if current.as_deref() != base_blob { return Err(GitError::Conflict); }
        let sig = Self::user_sig(author)?;
        let msg = Self::clean_message(message, format!("Modification de {slug}"));
        let oid = Self::commit_article(&repo, &head, MAIN_REF, slug, Some(blob), &sig, &sig, &msg)?;
        if oid == head.id() { return Err(GitError::Invalid("no_changes")); }
        Ok((oid.to_string(), blob.to_string()))
    }

    /// Supprime un article de la branche principale (`base_blob` : version que l'éditeur a vue).
    pub fn delete_article(&self, slug: &str, base_blob: &str, author: &str) -> GitResult<String> {
        if !valid_slug(slug) { return Err(GitError::NotFound); }
        let _guard = self.write_lock.lock().unwrap();
        let repo = self.repo()?;
        let head = repo.find_commit(self.head_oid()?)?;
        let current = self.blob_at(&repo, head.id(), slug)?.ok_or(GitError::NotFound)?;
        if current.to_string() != base_blob { return Err(GitError::Conflict); }
        let sig = Self::user_sig(author)?;
        let oid = Self::commit_article(&repo, &head, MAIN_REF, slug, None, &sig, &sig, &format!("Suppression de {slug}"))?;
        Ok(oid.to_string())
    }

    // ------------------------------------------------------------------ propositions

    pub fn proposal_ref(user: &str, slug: &str) -> GitResult<String> {
        if !valid_slug(slug) { return Err(GitError::Invalid("invalid_slug")); }
        let user_ok = !user.is_empty() && user.len() <= 32
            && user.bytes().next().is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            && user.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-'));
        let name = format!("refs/heads/users/{user}/{slug}");
        if !user_ok || !git2::Reference::is_valid_name(&name) { return Err(GitError::NotFound); }
        Ok(name)
    }

    /// Propositions en attente (celles d'un seul compte, ou de tous) : versions qui diffèrent de `main`.
    pub fn list_proposals(&self, only_user: Option<&str>) -> GitResult<Vec<Proposal>> {
        let repo = self.repo()?;
        let main = self.head_oid()?;
        let mut out = vec![];
        for r in repo.references_glob("refs/heads/users/*/*")?.flatten() {
            let Some(rest) = r.name().and_then(|n| n.strip_prefix("refs/heads/users/")) else { continue };
            let Some((user, slug)) = rest.split_once('/') else { continue };
            if only_user.is_some_and(|u| u != user) || Self::proposal_ref(user, slug).is_err() { continue; }
            let Some(tip) = r.target() else { continue };
            let Some(here) = self.blob_at(&repo, tip, slug)? else { continue };
            let in_main = self.blob_at(&repo, main, slug)?;
            if in_main == Some(here) { continue; }
            let commit = repo.find_commit(tip)?;
            let text = String::from_utf8_lossy(repo.find_blob(here)?.content()).into_owned();
            let meta = article::parse(&text).map(|(m, _)| m).unwrap_or_default();
            let title = meta.get("title_fr").or_else(|| meta.get("title_en")).cloned().unwrap_or_default();
            out.push(Proposal {
                user: user.to_string(),
                slug: slug.to_string(),
                commit: tip.to_string(),
                time: commit.time().seconds(),
                message: commit.message().unwrap_or("").lines().next().unwrap_or("").to_string(),
                title,
                is_new: in_main.is_none(),
            });
        }
        out.sort_by_key(|p| std::cmp::Reverse(p.time));
        Ok(out)
    }

    /// Contenu d'une proposition : (texte, version, version actuelle sur `main`).
    pub fn read_proposal(&self, user: &str, slug: &str) -> GitResult<(String, String, Option<String>)> {
        let name = Self::proposal_ref(user, slug)?;
        let repo = self.repo()?;
        let tip = repo.find_reference(&name).map_err(|_| GitError::NotFound)?.peel_to_commit()?;
        let blob = self.blob_at(&repo, tip.id(), slug)?.ok_or(GitError::NotFound)?;
        let content = String::from_utf8_lossy(repo.find_blob(blob)?.content()).into_owned();
        let main = self.blob_at(&repo, self.head_oid()?, slug)?.map(|o| o.to_string());
        Ok((content, blob.to_string(), main))
    }

    /// Enregistre une version sur la branche `users/<nom>/<slug>` (créée au besoin à partir de `main`).
    pub fn write_proposal(&self, user: &str, slug: &str, content: &str, base_blob: Option<&str>, message: &str) -> GitResult<(String, String)> {
        let name = Self::proposal_ref(user, slug)?;
        let _guard = self.write_lock.lock().unwrap();
        let repo = self.repo()?;
        let blob = self.checked_blob(&repo, slug, content)?;
        let existing = match repo.find_reference(&name) {
            Ok(r) => Some(r.peel_to_commit()?),
            Err(e) if e.code() == ErrorCode::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        let parent = match existing {
            Some(c) => c,
            None => {
                if self.list_proposals(Some(user))?.len() >= MAX_PROPOSALS_PER_USER { return Err(GitError::Invalid("too_many_proposals")); }
                repo.find_commit(self.head_oid()?)?
            }
        };
        let current = self.blob_at(&repo, parent.id(), slug)?.map(|o| o.to_string());
        if current.as_deref() != base_blob { return Err(GitError::Conflict); }
        let sig = Self::user_sig(user)?;
        let msg = Self::clean_message(message, format!("Proposition pour {slug}"));
        let oid = Self::commit_article(&repo, &parent, &name, slug, Some(blob), &sig, &sig, &msg)?;
        // Rien de nouveau : pas de branche vide, pas de commit.
        if oid == parent.id() { return Err(GitError::Invalid("no_changes")); }
        Ok((oid.to_string(), blob.to_string()))
    }

    /// Publie une proposition sur `main` (auteur : le proposant ; validateur : `editor`) puis supprime la branche.
    pub fn publish_proposal(&self, user: &str, slug: &str, main_base: Option<&str>, editor: &str) -> GitResult<String> {
        let name = Self::proposal_ref(user, slug)?;
        let _guard = self.write_lock.lock().unwrap();
        let repo = self.repo()?;
        let tip = repo.find_reference(&name).map_err(|_| GitError::NotFound)?.peel_to_commit()?;
        let blob = self.blob_at(&repo, tip.id(), slug)?.ok_or(GitError::NotFound)?;
        let content = String::from_utf8_lossy(repo.find_blob(blob)?.content()).into_owned();
        article::validate(slug, &content).map_err(GitError::Invalid)?;
        let head = repo.find_commit(self.head_oid()?)?;
        let current = self.blob_at(&repo, head.id(), slug)?.map(|o| o.to_string());
        if current.as_deref() != main_base { return Err(GitError::Conflict); }
        let author = Self::user_sig(user)?;
        let committer = Self::user_sig(editor)?;
        let first_line = tip.message().unwrap_or("").lines().next().unwrap_or("").trim().to_string();
        let msg = Self::clean_message(&format!("{first_line} (proposition de {user}, publiée par {editor})"), format!("Publication de {slug}"));
        let oid = Self::commit_article(&repo, &head, MAIN_REF, slug, Some(blob), &author, &committer, &msg)?;
        repo.find_reference(&name)?.delete()?;
        Ok(oid.to_string())
    }

    pub fn delete_proposal(&self, user: &str, slug: &str) -> GitResult<()> {
        let name = Self::proposal_ref(user, slug)?;
        let _guard = self.write_lock.lock().unwrap();
        let repo = self.repo()?;
        repo.find_reference(&name).map_err(|_| GitError::NotFound)?.delete()?;
        Ok(())
    }
}

fn update_tree(repo: &Repository, base: Option<&Tree>, comps: &[&str], blob: Oid) -> GitResult<Oid> {
    let mut tb = repo.treebuilder(base)?;
    if comps.len() == 1 {
        tb.insert(comps[0], blob, 0o100644)?;
    } else {
        let sub = base
            .and_then(|t| t.get_name(comps[0]))
            .and_then(|e| e.to_object(repo).ok())
            .and_then(|o| o.into_tree().ok());
        let id = update_tree(repo, sub.as_ref(), &comps[1..], blob)?;
        tb.insert(comps[0], id, 0o040000)?;
    }
    Ok(tb.write()?)
}

fn remove_from_tree(repo: &Repository, base: &Tree, comps: &[&str]) -> GitResult<Oid> {
    let mut tb = repo.treebuilder(Some(base))?;
    if comps.len() == 1 {
        tb.remove(comps[0]).map_err(|_| GitError::NotFound)?;
    } else {
        let sub = base
            .get_name(comps[0])
            .and_then(|e| e.to_object(repo).ok())
            .and_then(|o| o.into_tree().ok())
            .ok_or(GitError::NotFound)?;
        let id = remove_from_tree(repo, &sub, &comps[1..])?;
        tb.insert(comps[0], id, 0o040000)?;
    }
    Ok(tb.write()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn art(slug: &str, title: &str) -> String {
        format!("---\norder: 1\nslug: {slug}\nlang: fr\ntitle_fr: {title}\n---\n\nContenu.\n")
    }

    fn store() -> (GitStore, PathBuf) {
        let dir = std::env::temp_dir().join(format!("jp-git-test-{}-{}", std::process::id(), rand_suffix()));
        let seed = dir.join("seed");
        fs::create_dir_all(&seed).unwrap();
        fs::write(seed.join("demo.md"), art("demo", "Démo")).unwrap();
        (GitStore::open_or_init(&dir, &seed).unwrap(), dir)
    }

    /// Unique même quand plusieurs tests démarrent dans la même microseconde.
    fn rand_suffix() -> u128 {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        t * 1000 + u128::from(N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 1000)
    }

    #[test]
    fn lists_reads_writes_and_deletes() {
        let (git, dir) = store();
        let list = git.list_articles().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!((list[0].slug.as_str(), list[0].title_fr.as_str(), list[0].order), ("demo", "Démo", 1));
        let (text, v1) = git.read_article("demo", None).unwrap();
        assert!(text.contains("Contenu."));
        // Écriture avec la bonne version de base, puis conflit avec l'ancienne.
        let (_, v2) = git.write_article("demo", &art("demo", "Démo 2"), Some(&v1), "alice", "titre").unwrap();
        assert_ne!(v1, v2);
        assert!(matches!(git.write_article("demo", &art("demo", "Démo 3"), Some(&v1), "alice", ""), Err(GitError::Conflict)));
        // Création : la base doit être absente.
        git.write_article("neuf", &art("neuf", "Neuf"), None, "alice", "").unwrap();
        assert!(matches!(git.write_article("neuf", &art("neuf", "Neuf 2"), None, "alice", ""), Err(GitError::Conflict)));
        // Réécrire la même version ne crée rien.
        let (_, vnow) = git.read_article("demo", None).unwrap();
        let (same, _) = git.read_article("demo", None).unwrap();
        assert!(matches!(git.write_article("demo", &same, Some(&vnow), "alice", ""), Err(GitError::Invalid("no_changes"))));
        assert!(matches!(git.write_proposal("bob", "demo", &same, Some(&vnow), ""), Err(GitError::Invalid("no_changes"))));
        assert!(git.list_proposals(Some("bob")).unwrap().is_empty());
        // Un article invalide est refusé.
        assert!(matches!(git.write_article("neuf", "n'importe quoi", Some("x"), "alice", ""), Err(GitError::Invalid(_))));
        // Suppression.
        let (_, vn) = git.read_article("neuf", None).unwrap();
        assert!(matches!(git.delete_article("neuf", "mauvaise-version", "alice"), Err(GitError::Conflict)));
        git.delete_article("neuf", &vn, "alice").unwrap();
        assert!(matches!(git.read_article("neuf", None), Err(GitError::NotFound)));
        assert_eq!(git.history("demo", 10).unwrap().len(), 2);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn proposals_publish_and_cleanup() {
        let (git, dir) = store();
        let (_, main_v) = git.read_article("demo", None).unwrap();
        // Première version d'une proposition : la base est la version de main.
        let (c1, p1) = git.write_proposal("bob", "demo", &art("demo", "Proposé"), Some(&main_v), "ma proposition").unwrap();
        assert!(!c1.is_empty());
        // Mauvaise base → conflit ; bonne base (la version de la branche) → ok.
        assert!(matches!(git.write_proposal("bob", "demo", &art("demo", "Proposé 2"), Some(&main_v), ""), Err(GitError::Conflict)));
        let (_, p2) = git.write_proposal("bob", "demo", &art("demo", "Proposé 2"), Some(&p1), "").unwrap();
        // Une nouvelle proposition (article inexistant) part de rien.
        git.write_proposal("bob", "idee", &art("idee", "Idée"), None, "").unwrap();
        let mine = git.list_proposals(Some("bob")).unwrap();
        assert_eq!(mine.len(), 2);
        assert!(mine.iter().any(|p| p.slug == "idee" && p.is_new));
        assert!(git.list_proposals(Some("carole")).unwrap().is_empty());
        let (text, version, in_main) = git.read_proposal("bob", "demo").unwrap();
        assert!(text.contains("Proposé 2"));
        assert_eq!((version.as_str(), in_main.as_deref()), (p2.as_str(), Some(main_v.as_str())));
        // Publication : la base de main doit être celle qui a été relue.
        assert!(matches!(git.publish_proposal("bob", "demo", Some("autre"), "ed"), Err(GitError::Conflict)));
        git.publish_proposal("bob", "demo", Some(&main_v), "ed").unwrap();
        let (published, _) = git.read_article("demo", None).unwrap();
        assert!(published.contains("Proposé 2"));
        assert_eq!(git.list_proposals(Some("bob")).unwrap().len(), 1, "la branche publiée disparaît");
        assert_eq!(git.history("demo", 5).unwrap()[0].author, "bob");
        // Suppression d'une proposition, et noms de branches dangereux.
        git.delete_proposal("bob", "idee").unwrap();
        assert!(matches!(git.delete_proposal("bob", "idee"), Err(GitError::NotFound)));
        assert!(GitStore::proposal_ref("../x", "demo").is_err());
        assert!(GitStore::proposal_ref("bob", "a/b").is_err());
        let _ = fs::remove_dir_all(dir);
    }
}
