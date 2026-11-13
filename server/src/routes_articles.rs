//! API de lecture/écriture des articles, versionnés dans git.
use crate::{auth::{AuthUser, Shared}, error::{ApiError, ApiResult}, gitrepo::GitError};
use axum::{
    extract::{Path, State},
    routing::get,
    Json, Router,
};
use git2::Oid;
use serde::Deserialize;
use serde_json::{json, Value};

pub fn router() -> Router<Shared> {
    Router::new()
        .route("/articles", get(list))
        .route("/articles/{slug}", get(read).put(write))
        .route("/articles/{slug}/history", get(history))
        .route("/articles/{slug}/revisions/{commit}", get(revision))
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

async fn list(State(st): State<Shared>) -> ApiResult<Json<Value>> {
    let git = st.git.clone();
    let items = blocking(move || git.list_articles()).await?;
    Ok(Json(json!({ "articles": items.into_iter().map(|(slug, version)| json!({ "slug": slug, "version": version })).collect::<Vec<_>>() })))
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
    if !user.can_edit() { return Err(ApiError::forbidden("not_an_editor")); }
    let git = st.git.clone();
    let author = user.username.clone();
    let commit = blocking(move || git.write_article(&slug, &b.content, b.base.as_deref(), &author, &b.message)).await?;
    st.site.trigger();
    Ok(Json(json!({ "commit": commit })))
}

async fn site_status(State(st): State<Shared>) -> Json<Value> {
    Json(serde_json::to_value(st.site.status.lock().unwrap().clone()).unwrap_or_default())
}
