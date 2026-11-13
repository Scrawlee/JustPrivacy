//! API des articles (versionnés dans git) et des propositions de modification.
//! - Éditeurs et administrateurs écrivent directement sur `main` (le site est reconstruit).
//! - Les autres comptes enregistrent des propositions sur `users/<nom>/<slug>`, relues puis publiées par un éditeur.
use crate::{auth::{AuthUser, Shared}, error::{ApiError, ApiResult}, gitrepo::GitError};
use axum::{
    extract::{Path, Query, State},
    routing::{get, post, put},
    Json, Router,
};
use git2::Oid;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

pub fn router() -> Router<Shared> {
    Router::new()
        .route("/articles", get(list))
        .route("/articles/{slug}", get(read).put(write).delete(remove))
        .route("/articles/{slug}/history", get(history))
        .route("/articles/{slug}/revisions/{commit}", get(revision))
        .route("/proposals", get(proposals))
        .route("/proposals/{slug}", put(propose))
        .route("/proposals/{user}/{slug}", get(proposal).delete(withdraw))
        .route("/proposals/{user}/{slug}/publish", post(publish))
        .route("/site", get(site_status))
}

fn map(e: GitError) -> ApiError {
    match e {
        GitError::NotFound => ApiError::not_found(),
        GitError::Conflict => ApiError::conflict("edit_conflict"),
        GitError::Invalid(c) => ApiError::bad(c),
        GitError::Other(m) => ApiError::internal(m),
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, GitError> + Send + 'static) -> ApiResult<T> {
    tokio::task::spawn_blocking(f).await.map_err(ApiError::internal)?.map_err(map)
}

/// Écritures : 120 par heure et par compte.
fn throttle(st: &Shared, user: &AuthUser) -> ApiResult<()> {
    if st.limiter.hit(&format!("edit:{}", user.id), 120, Duration::from_secs(3600)) { Ok(()) } else { Err(ApiError::too_many()) }
}

fn require_editor(user: &AuthUser) -> ApiResult<()> {
    if user.can_edit() { Ok(()) } else { Err(ApiError::forbidden("not_an_editor")) }
}

/// Une proposition est visible et modifiable par son auteur et par les éditeurs.
fn require_owner_or_editor(user: &AuthUser, owner: &str) -> ApiResult<()> {
    if user.username == owner || user.can_edit() { Ok(()) } else { Err(ApiError::forbidden("not_your_proposal")) }
}

async fn list(State(st): State<Shared>) -> ApiResult<Json<Value>> {
    let git = st.git.clone();
    Ok(Json(json!({ "articles": blocking(move || git.list_articles()).await? })))
}

async fn read(State(st): State<Shared>, Path(slug): Path<String>) -> ApiResult<Json<Value>> {
    let git = st.git.clone();
    let (content, version) = blocking(move || git.read_article(&slug, None)).await?;
    Ok(Json(json!({ "content": content, "version": version })))
}

async fn history(State(st): State<Shared>, Path(slug): Path<String>) -> ApiResult<Json<Value>> {
    let git = st.git.clone();
    Ok(Json(json!({ "revisions": blocking(move || git.history(&slug, 100)).await? })))
}

async fn revision(State(st): State<Shared>, Path((slug, commit)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let oid = Oid::from_str(&commit).map_err(|_| ApiError::bad("invalid_commit"))?;
    let git = st.git.clone();
    let (content, version) = blocking(move || git.read_article(&slug, Some(oid))).await?;
    Ok(Json(json!({ "content": content, "version": version })))
}

#[derive(Deserialize)]
struct Write {
    content: String,
    /// Version (identifiant de blob) ouverte par l'éditeur ; absente pour créer un article.
    base: Option<String>,
    #[serde(default)]
    message: String,
}

async fn write(State(st): State<Shared>, user: AuthUser, Path(slug): Path<String>, Json(b): Json<Write>) -> ApiResult<Json<Value>> {
    require_editor(&user)?;
    throttle(&st, &user)?;
    let git = st.git.clone();
    let author = user.username.clone();
    let (commit, version) = blocking(move || git.write_article(&slug, &b.content, b.base.as_deref(), &author, &b.message)).await?;
    refresh(&st).await;
    Ok(Json(json!({ "commit": commit, "version": version })))
}

#[derive(Deserialize)]
struct Base {
    base: String,
}

async fn remove(State(st): State<Shared>, user: AuthUser, Path(slug): Path<String>, Query(q): Query<Base>) -> ApiResult<Json<Value>> {
    require_editor(&user)?;
    throttle(&st, &user)?;
    let git = st.git.clone();
    let author = user.username.clone();
    let commit = blocking(move || git.delete_article(&slug, &q.base, &author)).await?;
    refresh(&st).await;
    Ok(Json(json!({ "commit": commit })))
}

// ------------------------------------------------------------------ propositions

async fn proposals(State(st): State<Shared>, user: AuthUser) -> ApiResult<Json<Value>> {
    let git = st.git.clone();
    let all = user.can_edit();
    let me = user.username.clone();
    let items = blocking(move || git.list_proposals(if all { None } else { Some(me.as_str()) })).await?;
    Ok(Json(json!({ "scope": if all { "all" } else { "own" }, "proposals": items })))
}

async fn proposal(State(st): State<Shared>, user: AuthUser, Path((owner, slug)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    require_owner_or_editor(&user, &owner)?;
    let git = st.git.clone();
    let (content, version, main_version) = blocking(move || git.read_proposal(&owner, &slug)).await?;
    Ok(Json(json!({ "content": content, "version": version, "main_version": main_version })))
}

async fn propose(State(st): State<Shared>, user: AuthUser, Path(slug): Path<String>, Json(b): Json<Write>) -> ApiResult<Json<Value>> {
    throttle(&st, &user)?;
    let git = st.git.clone();
    let owner = user.username.clone();
    let (commit, version) = blocking(move || git.write_proposal(&owner, &slug, &b.content, b.base.as_deref(), &b.message)).await?;
    Ok(Json(json!({ "commit": commit, "version": version })))
}

#[derive(Deserialize)]
struct Publish {
    /// Version de l'article sur `main` que l'éditeur a relue (absente pour un nouvel article).
    main_base: Option<String>,
}

async fn publish(State(st): State<Shared>, user: AuthUser, Path((owner, slug)): Path<(String, String)>, Json(b): Json<Publish>) -> ApiResult<Json<Value>> {
    require_editor(&user)?;
    throttle(&st, &user)?;
    let git = st.git.clone();
    let editor = user.username.clone();
    let commit = blocking(move || git.publish_proposal(&owner, &slug, b.main_base.as_deref(), &editor)).await?;
    refresh(&st).await;
    Ok(Json(json!({ "commit": commit })))
}

async fn withdraw(State(st): State<Shared>, user: AuthUser, Path((owner, slug)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    require_owner_or_editor(&user, &owner)?;
    throttle(&st, &user)?;
    let git = st.git.clone();
    blocking(move || git.delete_proposal(&owner, &slug)).await?;
    Ok(Json(json!({ "ok": true })))
}

/// Les pages sont rendues à la demande : on purge ici celles dont un article vient de changer.
async fn refresh(st: &Shared) {
    let (pages, git) = (st.pages.clone(), st.git.clone());
    let _ = tokio::task::spawn_blocking(move || pages.sync(&git)).await;
}

async fn site_status(State(st): State<Shared>) -> Json<Value> {
    Json(json!({ "ok": true, "commit": st.pages.head(), "pages": st.pages.stats() }))
}
