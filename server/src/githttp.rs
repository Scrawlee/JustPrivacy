//! Serveur git « smart HTTP » écrit en Rust au-dessus de libgit2 : clone/fetch anonymes,
//! push authentifié par jeton personnel. Protocole v0 (les clients récents retombent dessus).
use crate::{auth::{client_ip, Shared}, crypto, gitrepo::{GitError, MAIN_REF}};
use axum::{
    body::Bytes,
    extract::{ConnectInfo, DefaultBodyLimit, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use git2::{Oid, Repository};
use serde::Deserialize;
use std::{io::{Read, Write}, net::SocketAddr, time::Duration};

pub fn router() -> Router<Shared> {
    Router::new()
        .route("/git/articles.git/info/refs", get(info_refs))
        .route("/git/articles.git/git-upload-pack", post(upload_pack))
        .route("/git/articles.git/git-receive-pack", post(receive_pack))
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
}

const AGENT: &str = "agent=justprivacy/0.1";
const ZERO: &str = "0000000000000000000000000000000000000000";

// ------------------------------------------------------------------ pkt-line

fn pkt(out: &mut Vec<u8>, data: &[u8]) {
    out.extend(format!("{:04x}", data.len() + 4).as_bytes());
    out.extend(data);
}
fn flush(out: &mut Vec<u8>) { out.extend(b"0000"); }

enum Pkt<'a> { Data(&'a [u8]), Flush }

fn read_pkt<'a>(buf: &'a [u8], pos: &mut usize) -> Option<Pkt<'a>> {
    let hdr = buf.get(*pos..*pos + 4)?;
    let len = usize::from_str_radix(std::str::from_utf8(hdr).ok()?, 16).ok()?;
    if len == 0 { *pos += 4; return Some(Pkt::Flush); }
    if len < 4 { return None; }
    let data = buf.get(*pos + 4..*pos + len)?;
    *pos += len;
    Some(Pkt::Data(data))
}

fn trim_nl(d: &[u8]) -> &[u8] { d.strip_suffix(b"\n").unwrap_or(d) }

fn git_response(content_type: &'static str, body: Vec<u8>) -> Response {
    (
        [(header::CONTENT_TYPE, content_type), (header::CACHE_CONTROL, "no-cache, max-age=0, must-revalidate")],
        body,
    )
        .into_response()
}

fn plain(status: StatusCode, msg: &'static str) -> Response { (status, msg).into_response() }

fn decode_body(headers: &HeaderMap, body: Bytes) -> Result<Vec<u8>, Response> {
    if headers.get(header::CONTENT_ENCODING).and_then(|v| v.to_str().ok()) == Some("gzip") {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(&body[..]).take(256 * 1024 * 1024).read_to_end(&mut out)
            .map_err(|_| plain(StatusCode::BAD_REQUEST, "gzip invalide"))?;
        Ok(out)
    } else {
        Ok(body.to_vec())
    }
}

// -------------------------------------------------------------------- accès

struct Pusher { id: uuid::Uuid, username: String, can_edit: bool }

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Basic realm=\"JustPrivacy git (nom d'utilisateur + jeton)\"")],
        "Authentification requise : utilisez votre nom d'utilisateur et un jeton d'accès personnel.\n",
    )
        .into_response()
}

/// Authentification HTTP Basic : nom d'utilisateur + jeton personnel (jamais le mot de passe du compte).
async fn authenticate(st: &Shared, headers: &HeaderMap, peer: &ConnectInfo<SocketAddr>) -> Result<Pusher, Response> {
    let ip = client_ip(headers, peer, st.cfg.trust_proxy_hops);
    if !st.limiter.hit(&format!("git:{ip}"), 120, Duration::from_secs(600)) {
        return Err(plain(StatusCode::TOO_MANY_REQUESTS, "Trop de requêtes.\n"));
    }
    let creds = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
        .and_then(|v| STANDARD.decode(v.trim()).ok())
        .and_then(|v| String::from_utf8(v).ok())
        .ok_or_else(unauthorized)?;
    let (user, token) = creds.split_once(':').ok_or_else(unauthorized)?;
    let row: Option<(uuid::Uuid, uuid::Uuid, String, String)> = sqlx::query_as(
        "SELECT t.id, u.id, u.username, u.role FROM api_tokens t JOIN users u ON u.id = t.user_id
         WHERE t.token_hash = $1 AND (t.expires_at IS NULL OR t.expires_at > now())",
    )
    .bind(crypto::sha256(token.as_bytes()))
    .fetch_optional(&st.db)
    .await
    .map_err(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "erreur\n"))?;
    let Some((token_id, id, username, role)) = row else { return Err(unauthorized()) };
    if !username.eq_ignore_ascii_case(user.trim()) { return Err(unauthorized()); }
    let _ = sqlx::query("UPDATE api_tokens SET last_used_at = now() WHERE id = $1").bind(token_id).execute(&st.db).await;
    Ok(Pusher { id, username, can_edit: matches!(role.as_str(), "editor" | "admin") })
}

// ------------------------------------------------------------------ info/refs

#[derive(Deserialize)]
struct ServiceQuery { service: Option<String> }

async fn info_refs(
    State(st): State<Shared>,
    Query(q): Query<ServiceQuery>,
    headers: HeaderMap,
    peer: ConnectInfo<SocketAddr>,
) -> Response {
    let service = match q.service.as_deref() {
        Some(s @ ("git-upload-pack" | "git-receive-pack")) => s.to_string(),
        _ => return plain(StatusCode::FORBIDDEN, "Seul le protocole smart HTTP est pris en charge.\n"),
    };
    let receive = service == "git-receive-pack";
    if receive {
        if let Err(r) = authenticate(&st, &headers, &peer).await { return r; }
    }
    let git = st.git.clone();
    let out = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, GitError> {
        let repo = git.repo()?;
        let mut refs: Vec<(String, Oid)> = vec![];
        let head = repo.refname_to_id(MAIN_REF).ok();
        for r in repo.references_glob("refs/heads/*")?.flatten() {
            if let (Some(n), Some(t)) = (r.name(), r.target()) { refs.push((n.to_string(), t)); }
        }
        refs.sort();
        let caps = if receive {
            format!("report-status delete-refs ofs-delta object-format=sha1 {AGENT}")
        } else {
            format!("multi_ack multi_ack_detailed side-band-64k ofs-delta no-progress object-format=sha1 symref=HEAD:{MAIN_REF} {AGENT}")
        };
        let mut out = Vec::new();
        pkt(&mut out, format!("# service={}\n", if receive { "git-receive-pack" } else { "git-upload-pack" }).as_bytes());
        flush(&mut out);
        let mut all: Vec<(String, String)> = vec![];
        if !receive {
            if let Some(h) = head { all.push(("HEAD".into(), h.to_string())); }
        }
        all.extend(refs.iter().map(|(n, t)| (n.clone(), t.to_string())));
        if all.is_empty() { all.push(("capabilities^{}".into(), ZERO.into())); }
        for (i, (name, oid)) in all.iter().enumerate() {
            let text = if i == 0 { format!("{oid} {name}\0{caps}\n") } else { format!("{oid} {name}\n") };
            pkt(&mut out, text.as_bytes());
        }
        flush(&mut out);
        Ok(out)
    })
    .await;
    match out {
        Ok(Ok(body)) => git_response(
            if receive { "application/x-git-receive-pack-advertisement" } else { "application/x-git-upload-pack-advertisement" },
            body,
        ),
        _ => plain(StatusCode::INTERNAL_SERVER_ERROR, "erreur\n"),
    }
}

// ---------------------------------------------------------------- upload-pack

async fn upload_pack(State(st): State<Shared>, headers: HeaderMap, body: Bytes) -> Response {
    let body = match decode_body(&headers, body) { Ok(b) => b, Err(r) => return r };
    let git = st.git.clone();
    let res = tokio::task::spawn_blocking(move || serve_fetch(&git.repo()?, &body)).await;
    match res {
        Ok(Ok(out)) => git_response("application/x-git-upload-pack-result", out),
        Ok(Err(GitError::Invalid(m))) => plain(StatusCode::BAD_REQUEST, m),
        _ => plain(StatusCode::INTERNAL_SERVER_ERROR, "erreur\n"),
    }
}

fn serve_fetch(repo: &Repository, body: &[u8]) -> Result<Vec<u8>, GitError> {
    let mut pos = 0;
    let mut wants: Vec<Oid> = vec![];
    let (mut sideband, mut done) = (false, false);
    // Demandes : « want <oid> [capacités] », puis un flush.
    loop {
        match read_pkt(body, &mut pos).ok_or(GitError::Invalid("requête invalide\n"))? {
            Pkt::Flush => break,
            Pkt::Data(d) => {
                let line = String::from_utf8_lossy(trim_nl(d)).into_owned();
                if let Some(rest) = line.strip_prefix("want ") {
                    let mut it = rest.split(' ');
                    wants.push(Oid::from_str(it.next().unwrap_or("")).map_err(|_| GitError::Invalid("want invalide\n"))?);
                    if it.any(|c| c == "side-band-64k") { sideband = true; }
                }
            }
        }
    }
    if wants.is_empty() { return Ok(b"0000".to_vec()); }
    // Négociation : « have <oid> » et « done ».
    let mut out = Vec::new();
    let mut common: Vec<Oid> = vec![];
    while pos < body.len() {
        match read_pkt(body, &mut pos).ok_or(GitError::Invalid("requête invalide\n"))? {
            Pkt::Flush => {}
            Pkt::Data(d) => {
                let line = String::from_utf8_lossy(trim_nl(d)).into_owned();
                if let Some(h) = line.strip_prefix("have ") {
                    if let Ok(oid) = Oid::from_str(h.trim()) {
                        if repo.find_commit(oid).is_ok() {
                            common.push(oid);
                            pkt(&mut out, format!("ACK {oid} common\n").as_bytes());
                        }
                    }
                } else if line == "done" {
                    done = true;
                }
            }
        }
    }
    if !done {
        // Étape de négociation seule : on dit au client qu'il en a assez dit.
        if let Some(last) = common.last() { pkt(&mut out, format!("ACK {last} ready\n").as_bytes()); }
        pkt(&mut out, b"NAK\n");
        return Ok(out);
    }
    match common.last() {
        Some(last) => pkt(&mut out, format!("ACK {last}\n").as_bytes()),
        None => pkt(&mut out, b"NAK\n"),
    }
    let mut walk = repo.revwalk()?;
    for w in &wants {
        let c = repo.find_commit(*w).map_err(|_| GitError::Invalid("objet demandé inconnu\n"))?;
        walk.push(c.id())?;
    }
    for c in &common { walk.hide(*c)?; }
    let mut pb = repo.packbuilder()?;
    pb.insert_walk(&mut walk)?;
    let mut pack = Vec::new();
    pb.foreach(|chunk| { pack.extend_from_slice(chunk); true })?;
    if sideband {
        for chunk in pack.chunks(65515) {
            let mut data = vec![1u8];
            data.extend_from_slice(chunk);
            pkt(&mut out, &data);
        }
        flush(&mut out);
    } else {
        out.extend(pack);
    }
    Ok(out)
}

// --------------------------------------------------------------- receive-pack

async fn receive_pack(State(st): State<Shared>, headers: HeaderMap, peer: ConnectInfo<SocketAddr>, body: Bytes) -> Response {
    let pusher = match authenticate(&st, &headers, &peer).await { Ok(p) => p, Err(r) => return r };
    let body = match decode_body(&headers, body) { Ok(b) => b, Err(r) => return r };
    let git = st.git.clone();
    let res = tokio::task::spawn_blocking(move || serve_push(&git, &pusher, &body)).await;
    match res {
        Ok(Ok((out, main_changed))) => {
            if main_changed {
                let (pages, git) = (st.pages.clone(), st.git.clone());
                tokio::task::spawn_blocking(move || { let _ = pages.sync(&git); });
            }
            git_response("application/x-git-receive-pack-result", out)
        }
        Ok(Err(GitError::Invalid(m))) => plain(StatusCode::BAD_REQUEST, m),
        _ => plain(StatusCode::INTERNAL_SERVER_ERROR, "erreur\n"),
    }
}

struct Command { old: Oid, new: Oid, name: String }

/// Droits de push : les éditeurs écrivent partout (main en avance rapide seulement),
/// les autres comptes uniquement dans `refs/heads/users/<nom>/…`.
fn may_update(p: &Pusher, name: &str) -> Result<(), &'static str> {
    if !name.starts_with("refs/heads/") || !git2::Reference::is_valid_name(name) {
        return Err("seules les branches sont acceptées");
    }
    if p.can_edit { return Ok(()); }
    if name.starts_with(&format!("refs/heads/users/{}/", p.username)) { Ok(()) } else {
        Err("compte sans droit d'édition : poussez vers users/<votre-nom>/…")
    }
}

fn serve_push(git: &crate::gitrepo::GitStore, pusher: &Pusher, body: &[u8]) -> Result<(Vec<u8>, bool), GitError> {
    let mut pos = 0;
    let mut cmds = vec![];
    loop {
        match read_pkt(body, &mut pos).ok_or(GitError::Invalid("requête invalide\n"))? {
            Pkt::Flush => break,
            Pkt::Data(d) => {
                let line = String::from_utf8_lossy(trim_nl(d)).into_owned();
                let line = line.split('\0').next().unwrap_or("").to_string();
                let mut it = line.splitn(3, ' ');
                let (old, new, name) = (it.next().unwrap_or(""), it.next().unwrap_or(""), it.next().unwrap_or(""));
                cmds.push(Command {
                    old: Oid::from_str(old).map_err(|_| GitError::Invalid("commande invalide\n"))?,
                    new: Oid::from_str(new).map_err(|_| GitError::Invalid("commande invalide\n"))?,
                    name: name.to_string(),
                });
            }
        }
    }
    let pack = &body[pos..];
    let _guard = git.write_lock.lock().unwrap();
    let repo = git.repo()?;
    let mut report = Vec::new();
    let mut main_changed = false;

    // Réception de l'objet pack (indexer de libgit2, packs « thin » complétés depuis la base).
    let mut unpack_ok = true;
    if !pack.is_empty() {
        let res = (|| -> Result<(), git2::Error> {
            let odb = repo.odb()?;
            let mut w = odb.packwriter()?;
            w.write_all(pack).map_err(|e| git2::Error::from_str(&e.to_string()))?;
            w.commit()?;
            Ok(())
        })();
        if let Err(e) = res {
            tracing::info!("pack refusé : {e}");
            unpack_ok = false;
        }
    }
    pkt(&mut report, if unpack_ok { b"unpack ok\n".to_vec() } else { b"unpack error\n".to_vec() }.as_slice());

    for c in &cmds {
        let verdict: Result<(), String> = (|| {
            if !unpack_ok { return Err("échec de réception du pack".into()); }
            may_update(pusher, &c.name).map_err(String::from)?;
            let current = repo.refname_to_id(&c.name).ok();
            let expected = (!c.old.is_zero()).then_some(c.old);
            if current != expected { return Err("la référence a changé entre-temps (fetch puis réessayez)".into()); }
            if c.new.is_zero() {
                if c.name == MAIN_REF { return Err("la branche principale ne peut pas être supprimée".into()); }
                repo.find_reference(&c.name).and_then(|mut r| r.delete()).map_err(|e| e.message().to_string())?;
                return Ok(());
            }
            repo.find_commit(c.new).map_err(|_| "commit inconnu".to_string())?;
            if c.name == MAIN_REF {
                if let Some(cur) = current {
                    if cur != c.new && !repo.graph_descendant_of(c.new, cur).unwrap_or(false) {
                        return Err("push non fast-forward refusé sur main".into());
                    }
                }
                main_changed = true;
            }
            repo.reference(&c.name, c.new, true, &format!("push par {}", pusher.id)).map_err(|e| e.message().to_string())?;
            Ok(())
        })();
        match verdict {
            Ok(()) => pkt(&mut report, format!("ok {}\n", c.name).as_bytes()),
            Err(why) => {
                main_changed = main_changed && c.name != MAIN_REF;
                pkt(&mut report, format!("ng {} {}\n", c.name, why).as_bytes())
            }
        }
    }
    flush(&mut report);
    Ok((report, main_changed))
}
