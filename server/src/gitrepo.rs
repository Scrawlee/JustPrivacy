//! Dépôt git des articles (bare, géré par libgit2) : lecture, historique, écriture, export.
use git2::{ErrorCode, ObjectType, Oid, Repository, RepositoryInitOptions, Signature, Sort, Tree};
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

    /// Liste (slug, identifiant du blob) des articles de la branche principale.
    pub fn list_articles(&self) -> GitResult<Vec<(String, String)>> {
        let repo = self.repo()?;
        let Some(tree) = self.articles_tree(&repo, self.head_oid()?)? else { return Ok(vec![]) };
        Ok(tree
            .iter()
            .filter(|e| e.kind() == Some(ObjectType::Blob))
            .filter_map(|e| {
                let name = e.name()?;
                Some((name.strip_suffix(".md")?.to_string(), e.id().to_string()))
            })
            .collect())
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

    /// Crée ou modifie un article. `base_blob` est la version que l'auteur a lue (None pour une création).
    pub fn write_article(&self, slug: &str, content: &str, base_blob: Option<&str>, author: &str, message: &str) -> GitResult<String> {
        if !valid_slug(slug) { return Err(GitError::Invalid("invalid_slug")); }
        if content.len() > MAX_ARTICLE_BYTES { return Err(GitError::Invalid("too_large")); }
        if !content.starts_with("---") { return Err(GitError::Invalid("missing_front_matter")); }
        let _guard = self.write_lock.lock().unwrap();
        let repo = self.repo()?;
        let head = repo.find_commit(self.head_oid()?)?;
        let current = self.blob_at(&repo, head.id(), slug)?.map(|o| o.to_string());
        if current.as_deref() != base_blob { return Err(GitError::Conflict); }
        let blob = repo.blob(content.as_bytes())?;
        let path = [ARTICLES_DIR[0], ARTICLES_DIR[1], &format!("{slug}.md")];
        let new_tree = update_tree(&repo, Some(&head.tree()?), &path, blob)?;
        let tree = repo.find_tree(new_tree)?;
        if tree.id() == head.tree_id() { return Ok(head.id().to_string()); }
        let sig = Signature::now(author, &format!("{author}@users.noreply.justprivacy.invalid"))?;
        let message = if message.trim().is_empty() { format!("Modification de {slug}") } else { message.trim().chars().take(500).collect() };
        let oid = repo.commit(Some(MAIN_REF), &sig, &sig, &message, &tree, &[&head])?;
        Ok(oid.to_string())
    }

    /// Écrit les articles de la branche principale dans `dest` (pour la construction du site).
    pub fn export_articles(&self, dest: &Path) -> GitResult<Oid> {
        let repo = self.repo()?;
        let head = self.head_oid()?;
        fs::create_dir_all(dest)?;
        if let Some(tree) = self.articles_tree(&repo, head)? {
            for e in tree.iter().filter(|e| e.kind() == Some(ObjectType::Blob)) {
                let Some(name) = e.name() else { continue };
                if name.ends_with(".md") && !name.contains('/') && name != ".md" {
                    fs::write(dest.join(name), repo.find_blob(e.id())?.content())?;
                }
            }
        }
        Ok(head)
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
