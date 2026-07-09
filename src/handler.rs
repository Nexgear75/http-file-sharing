//! Gestion des requêtes : résolution de chemin sécurisée, service de fichiers
//! statiques (avec Range + suivi de progression), listing de répertoire, upload
//! multipart et téléchargement de dossier en zip à la volée.

use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::{ConnectInfo, Multipart, Query, State};
use axum::http::{header, HeaderMap, Method, StatusCode, Uri};
use axum::response::{Html, IntoResponse, Response};
use bytes::Bytes;
use futures::SinkExt;
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio_util::io::ReaderStream;

use crate::error::error_response;
use crate::listing::{self, Entry};
use crate::progress::Direction;
use crate::state::AppState;
use crate::template::{self, Crumb};

/// Au-delà de cette taille, un téléchargement obtient une barre de progression
/// dédiée ; en-dessous, il est servi en mémoire et logué en une ligne.
const BAR_THRESHOLD: u64 = 512 * 1024;

/// Taille des chunks lors du streaming de fichiers.
const CHUNK_SIZE: usize = 64 * 1024;

/// Paramètres de query pris en charge.
#[derive(Debug, Deserialize)]
pub struct ServeQuery {
    /// `?zip=1` sur un dossier : télécharge le dossier en zip.
    zip: Option<u8>,
}

/// Endpoint de healthcheck (utilisé par l'indicateur de connexion du front).
pub async fn health() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

/// Handler principal : sert fichiers, listings et zip pour GET/HEAD.
pub async fn serve(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    method: Method,
    uri: Uri,
    Query(q): Query<ServeQuery>,
    headers: HeaderMap,
) -> Response {
    if method != Method::GET && method != Method::HEAD {
        return error_response(StatusCode::METHOD_NOT_ALLOWED);
    }

    let start = Instant::now();
    let ip = addr.ip().to_string();
    let decoded = percent_encoding::percent_decode_str(uri.path())
        .decode_utf8_lossy()
        .to_string();

    let resolved = match resolve_path(&state.config.root, &decoded) {
        Some(p) => p,
        None => {
            state
                .tracker
                .log_request("GET", &decoded, &ip, 403, start.elapsed(), 0);
            return error_response(StatusCode::FORBIDDEN);
        }
    };

    let metadata = match tokio::fs::metadata(&resolved).await {
        Ok(m) => m,
        Err(_) => {
            state
                .tracker
                .log_request("GET", &decoded, &ip, 404, start.elapsed(), 0);
            return error_response(StatusCode::NOT_FOUND);
        }
    };

    if metadata.is_dir() {
        // Téléchargement du dossier en zip.
        if q.zip == Some(1) {
            return serve_zip(&state, &resolved, &decoded, ip).await;
        }

        // index.html s'il existe.
        let index = resolved.join("index.html");
        if tokio::fs::metadata(&index).await.map(|m| m.is_file()).unwrap_or(false) {
            return serve_file(&state, &index, &decoded, &ip, &method, &headers, start).await;
        }

        if state.config.no_index {
            state
                .tracker
                .log_request("GET", &decoded, &ip, 404, start.elapsed(), 0);
            return error_response(StatusCode::NOT_FOUND);
        }

        return serve_listing(&state, &resolved, &decoded, &ip, start).await;
    }

    serve_file(&state, &resolved, &decoded, &ip, &method, &headers, start).await
}

/// Résout un chemin URL en chemin disque, en refusant tout échappement hors de
/// la racine et tout composant caché (préfixe `.`).
fn resolve_path(root: &Path, url_path: &str) -> Option<PathBuf> {
    let trimmed = url_path.trim_start_matches('/');
    let rel = Path::new(trimmed);

    let mut safe = PathBuf::new();
    for comp in rel.components() {
        match comp {
            Component::Normal(part) => {
                let s = part.to_string_lossy();
                if s.starts_with('.') {
                    return None; // fichiers/dossiers cachés interdits
                }
                safe.push(part);
            }
            Component::CurDir => {}
            // ParentDir, RootDir, Prefix : tentative de traversal → refus.
            _ => return None,
        }
    }

    let full = root.join(&safe);
    // Canonicalisation défensive : le chemin réel doit rester sous la racine.
    match full.canonicalize() {
        Ok(canon) => {
            if canon.starts_with(root) {
                Some(canon)
            } else {
                None
            }
        }
        // Le fichier peut ne pas exister (404 géré plus haut) : on retourne le
        // chemin joint, déjà purgé de tout `..`.
        Err(_) => Some(full),
    }
}

/// Sert un fichier statique avec Content-Type, ETag, Last-Modified, support des
/// requêtes Range et suivi de progression pour les gros transferts.
async fn serve_file(
    state: &AppState,
    path: &Path,
    url_path: &str,
    ip: &str,
    method: &Method,
    headers: &HeaderMap,
    start: Instant,
) -> Response {
    let metadata = match tokio::fs::metadata(path).await {
        Ok(m) => m,
        Err(_) => return error_response(StatusCode::NOT_FOUND),
    };
    let total_len = metadata.len();

    let mime = mime_guess::from_path(path)
        .first_or_octet_stream()
        .to_string();
    let modified = metadata.modified().ok();
    let etag = compute_etag(total_len, modified);
    let last_modified = modified.map(httpdate_like);

    // 304 Not Modified.
    if let Some(inm) = headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()) {
        if inm.split(',').any(|t| t.trim() == etag) {
            state
                .tracker
                .log_request(method.as_str(), url_path, ip, 304, start.elapsed(), 0);
            return build_not_modified(&etag);
        }
    }

    // Analyse d'un éventuel Range.
    let (start_byte, end_byte, is_partial) =
        match parse_range(headers, total_len) {
            RangeResult::Full => (0, total_len.saturating_sub(1), false),
            RangeResult::Partial(s, e) => (s, e, true),
            RangeResult::Unsatisfiable => {
                return (
                    StatusCode::RANGE_NOT_SATISFIABLE,
                    [(header::CONTENT_RANGE, format!("bytes */{total_len}"))],
                )
                    .into_response();
            }
        };
    let length = end_byte.saturating_sub(start_byte) + 1;

    let status = if is_partial {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };

    // En-têtes communs.
    // `X-File-Size` = nombre d'octets *décompressés* de ce transfert. Le
    // navigateur s'en sert pour la barre de progression même quand la
    // compression retire `Content-Length` (réponse chunked). Cela garantit une
    // barre web synchronisée avec la barre console (mêmes octets / même total).
    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CONTENT_LENGTH, length)
        .header("x-file-size", length)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::ETAG, etag.clone())
        .header(header::CACHE_CONTROL, "public, max-age=3600");
    if let Some(lm) = &last_modified {
        builder = builder.header(header::LAST_MODIFIED, lm);
    }
    if is_partial {
        builder = builder.header(
            header::CONTENT_RANGE,
            format!("bytes {start_byte}-{end_byte}/{total_len}"),
        );
    }

    // Requête HEAD : pas de corps.
    if method == Method::HEAD {
        state
            .tracker
            .log_request("HEAD", url_path, ip, status.as_u16(), start.elapsed(), 0);
        return builder.body(Body::empty()).unwrap_or_else(|_| {
            error_response(StatusCode::INTERNAL_SERVER_ERROR)
        });
    }

    // Petits fichiers : lecture en mémoire + log en une ligne.
    if length < BAR_THRESHOLD {
        match read_range(path, start_byte, length).await {
            Ok(data) => {
                state.tracker.log_request(
                    "GET",
                    url_path,
                    ip,
                    status.as_u16(),
                    start.elapsed(),
                    length,
                );
                builder.body(Body::from(data)).unwrap_or_else(|_| {
                    error_response(StatusCode::INTERNAL_SERVER_ERROR)
                })
            }
            Err(_) => error_response(StatusCode::INTERNAL_SERVER_ERROR),
        }
    } else {
        // Gros fichiers : streaming avec barre de progression.
        let body = stream_with_progress(state, path, start_byte, length, url_path, ip, status.as_u16());
        builder
            .body(body)
            .unwrap_or_else(|_| error_response(StatusCode::INTERNAL_SERVER_ERROR))
    }
}

/// Lit `length` octets à partir de `offset` dans le fichier.
async fn read_range(path: &Path, offset: u64, length: u64) -> std::io::Result<Vec<u8>> {
    let mut file = tokio::fs::File::open(path).await?;
    if offset > 0 {
        file.seek(std::io::SeekFrom::Start(offset)).await?;
    }
    let mut buf = vec![0u8; length as usize];
    file.read_exact(&mut buf).await?;
    Ok(buf)
}

/// Construit un corps de réponse streamé qui alimente une barre de progression
/// console pendant l'envoi.
fn stream_with_progress(
    state: &AppState,
    path: &Path,
    offset: u64,
    length: u64,
    url_path: &str,
    ip: &str,
    status: u16,
) -> Body {
    let (mut tx, rx) = futures::channel::mpsc::channel::<Result<Bytes, std::io::Error>>(4);
    let handle = state.tracker.start_transfer(
        Direction::Download,
        "GET",
        url_path,
        ip.to_string(),
        length,
    );
    let path = path.to_path_buf();

    tokio::spawn(async move {
        let mut file = match tokio::fs::File::open(&path).await {
            Ok(f) => f,
            Err(e) => {
                let _ = tx.send(Err(e)).await;
                return;
            }
        };
        if offset > 0 {
            if let Err(e) = file.seek(std::io::SeekFrom::Start(offset)).await {
                let _ = tx.send(Err(e)).await;
                return;
            }
        }

        let mut remaining = length;
        let mut buf = vec![0u8; CHUNK_SIZE];
        while remaining > 0 {
            let to_read = (buf.len() as u64).min(remaining) as usize;
            match file.read(&mut buf[..to_read]).await {
                Ok(0) => break,
                Ok(n) => {
                    handle.inc(n as u64);
                    if tx
                        .send(Ok(Bytes::copy_from_slice(&buf[..n])))
                        .await
                        .is_err()
                    {
                        // Client déconnecté : Drop du handle → nettoyage.
                        return;
                    }
                    remaining -= n as u64;
                }
                Err(e) => {
                    let _ = tx.send(Err(e)).await;
                    return;
                }
            }
        }
        handle.finish(status);
    });

    Body::from_stream(rx)
}

/// Génère et sert la page de listing d'un répertoire.
async fn serve_listing(
    state: &AppState,
    dir: &Path,
    url_path: &str,
    ip: &str,
    start: Instant,
) -> Response {
    let url_prefix = url_path.trim_matches('/');
    let entries: Vec<Entry> = match listing::read_dir(dir, url_prefix).await {
        Ok(e) => e,
        Err(_) => return error_response(StatusCode::INTERNAL_SERVER_ERROR),
    };

    let dir_name = state
        .config
        .root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "/".to_string());

    let crumbs = build_breadcrumbs(&dir_name, url_path);
    let html = template::render(
        &dir_name,
        &crumbs,
        &entries,
        state.config.upload,
        &state.config.version,
    );

    state
        .tracker
        .log_request("GET", url_path, ip, 200, start.elapsed(), html.len() as u64);

    (StatusCode::OK, Html(html)).into_response()
}

/// Construit le fil d'Ariane à partir du chemin URL.
fn build_breadcrumbs(root_name: &str, url_path: &str) -> Vec<Crumb> {
    let mut crumbs = vec![Crumb {
        name: format!("📁 {root_name}"),
        url: "/".to_string(),
    }];
    let mut acc = String::new();
    for seg in url_path.split('/').filter(|s| !s.is_empty()) {
        acc.push('/');
        acc.push_str(seg);
        crumbs.push(Crumb {
            name: seg.to_string(),
            url: format!("{acc}/"),
        });
    }
    crumbs
}

/// Télécharge un dossier entier sous forme de zip streamé (sans écriture disque).
async fn serve_zip(state: &AppState, dir: &Path, url_path: &str, ip: String) -> Response {
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    collect_files(dir, dir, &mut files);

    let estimated: u64 = files
        .iter()
        .filter_map(|(_, p)| std::fs::metadata(p).ok().map(|m| m.len()))
        .sum();

    let zip_name = dir
        .file_name()
        .map(|n| format!("{}.zip", n.to_string_lossy()))
        .unwrap_or_else(|| "archive.zip".to_string());

    let (w, r) = tokio::io::duplex(CHUNK_SIZE);
    let handle = state.tracker.start_transfer(
        Direction::Download,
        "GET",
        url_path,
        ip,
        estimated.max(1),
    );

    tokio::spawn(async move {
        use async_zip::tokio::write::ZipFileWriter;
        use async_zip::{Compression, ZipEntryBuilder};

        let mut writer = ZipFileWriter::with_tokio(w);
        for (name, abs) in files {
            let data = match tokio::fs::read(&abs).await {
                Ok(d) => d,
                Err(_) => continue,
            };
            // « Stored » = aucune compression : vitesse disque/réseau, coût CPU
            // nul. Le zip sert uniquement de conteneur pour regrouper les
            // fichiers en un seul téléchargement.
            let builder = ZipEntryBuilder::new(name.into(), Compression::Stored);
            if writer.write_entry_whole(builder, &data).await.is_err() {
                return; // client déconnecté
            }
            handle.inc(data.len() as u64);
        }
        let _ = writer.close().await;
        handle.finish(200);
    });

    let stream = ReaderStream::new(r);
    let disposition = format!("attachment; filename=\"{zip_name}\"");
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/zip")
        .header(header::CONTENT_DISPOSITION, disposition)
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| error_response(StatusCode::INTERNAL_SERVER_ERROR))
}

/// Collecte récursivement les fichiers d'un dossier (chemins relatifs pour le
/// zip), en ignorant les fichiers cachés.
fn collect_files(base: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => return,
    };
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            collect_files(base, &path, out);
        } else if let Ok(rel) = path.strip_prefix(base) {
            out.push((rel.to_string_lossy().replace('\\', "/"), path.clone()));
        }
    }
}

// ---------------------------------------------------------------------------
// Upload
// ---------------------------------------------------------------------------

/// Query pour l'upload : sous-dossier de destination optionnel.
#[derive(Debug, Deserialize)]
pub struct UploadQuery {
    p: Option<String>,
}

/// Reçoit un ou plusieurs fichiers via multipart et les enregistre sur disque,
/// avec suivi de progression console.
pub async fn upload(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(q): Query<UploadQuery>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response {
    if !state.config.upload {
        return error_response(StatusCode::FORBIDDEN);
    }

    let ip = addr.ip().to_string();
    let dest_dir = match q.p.as_deref() {
        Some(sub) if !sub.is_empty() => match resolve_path(&state.config.root, sub) {
            Some(d) => d,
            None => return error_response(StatusCode::FORBIDDEN),
        },
        _ => state.config.root.clone(),
    };

    let total = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);

    let handle = state.tracker.start_transfer(
        Direction::Upload,
        "POST",
        "/__upload",
        ip.clone(),
        total.max(1),
    );

    let mut saved: Vec<String> = Vec::new();
    let mut received: u64 = 0;

    loop {
        let field = match multipart.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(_) => {
                handle.finish(400);
                return error_response(StatusCode::BAD_REQUEST);
            }
        };

        let filename = match field.file_name() {
            Some(name) => sanitize_filename(name),
            None => continue, // champ non-fichier
        };
        if filename.is_empty() {
            continue;
        }

        let target = dest_dir.join(&filename);
        let mut file = match tokio::fs::File::create(&target).await {
            Ok(f) => f,
            Err(_) => {
                handle.finish(500);
                return error_response(StatusCode::INTERNAL_SERVER_ERROR);
            }
        };

        let mut field = field;
        loop {
            match field.chunk().await {
                Ok(Some(chunk)) => {
                    received += chunk.len() as u64;
                    if received > state.config.max_upload_bytes {
                        handle.finish(413);
                        let _ = tokio::fs::remove_file(&target).await;
                        return error_response(StatusCode::PAYLOAD_TOO_LARGE);
                    }
                    handle.inc(chunk.len() as u64);
                    if file.write_all(&chunk).await.is_err() {
                        handle.finish(500);
                        return error_response(StatusCode::INTERNAL_SERVER_ERROR);
                    }
                }
                Ok(None) => break,
                Err(_) => {
                    handle.finish(400);
                    return error_response(StatusCode::BAD_REQUEST);
                }
            }
        }
        let _ = file.flush().await;
        saved.push(filename);
    }

    handle.finish(200);

    let body = serde_json::json!({ "ok": true, "files": saved });
    (StatusCode::OK, axum::Json(body)).into_response()
}

/// Réduit un nom de fichier à sa composante finale, sans séparateurs de chemin.
fn sanitize_filename(name: &str) -> String {
    let base = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .trim();
    if base == "." || base == ".." || base.is_empty() {
        String::new()
    } else {
        base.to_string()
    }
}

// ---------------------------------------------------------------------------
// Helpers ETag / Range / Last-Modified
// ---------------------------------------------------------------------------

fn compute_etag(len: u64, modified: Option<SystemTime>) -> String {
    let mtime = modified
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("\"{len:x}-{mtime:x}\"")
}

/// Formate un SystemTime en date HTTP (RFC 1123, UTC).
fn httpdate_like(t: SystemTime) -> String {
    let dt: chrono::DateTime<chrono::Utc> = t.into();
    dt.format("%a, %d %b %Y %H:%M:%S GMT").to_string()
}

fn build_not_modified(etag: &str) -> Response {
    Response::builder()
        .status(StatusCode::NOT_MODIFIED)
        .header(header::ETAG, etag)
        .body(Body::empty())
        .unwrap_or_else(|_| error_response(StatusCode::INTERNAL_SERVER_ERROR))
}

enum RangeResult {
    Full,
    Partial(u64, u64),
    Unsatisfiable,
}

/// Parse un header `Range: bytes=start-end` (une seule plage prise en charge).
fn parse_range(headers: &HeaderMap, total: u64) -> RangeResult {
    let Some(value) = headers.get(header::RANGE).and_then(|v| v.to_str().ok()) else {
        return RangeResult::Full;
    };
    let Some(spec) = value.strip_prefix("bytes=") else {
        return RangeResult::Full;
    };
    // On ne gère que la première plage.
    let spec = spec.split(',').next().unwrap_or("").trim();
    let Some((s, e)) = spec.split_once('-') else {
        return RangeResult::Unsatisfiable;
    };

    if total == 0 {
        return RangeResult::Unsatisfiable;
    }

    let (start, end) = match (s.trim(), e.trim()) {
        // bytes=-N : les N derniers octets.
        ("", end) => {
            let n: u64 = end.parse().unwrap_or(0);
            if n == 0 {
                return RangeResult::Unsatisfiable;
            }
            let n = n.min(total);
            (total - n, total - 1)
        }
        // bytes=N- : depuis N jusqu'à la fin.
        (start, "") => {
            let s: u64 = match start.parse() {
                Ok(v) => v,
                Err(_) => return RangeResult::Unsatisfiable,
            };
            (s, total - 1)
        }
        // bytes=A-B
        (start, end) => {
            let s: u64 = match start.parse() {
                Ok(v) => v,
                Err(_) => return RangeResult::Unsatisfiable,
            };
            let e: u64 = match end.parse() {
                Ok(v) => v,
                Err(_) => return RangeResult::Unsatisfiable,
            };
            (s, e.min(total - 1))
        }
    };

    if start > end || start >= total {
        RangeResult::Unsatisfiable
    } else {
        RangeResult::Partial(start, end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_path_blocks_traversal() {
        let root = std::env::current_dir().unwrap();
        // `..` explicite refusé.
        assert!(resolve_path(&root, "/../etc/passwd").is_none());
        assert!(resolve_path(&root, "/foo/../../bar").is_none());
        // Fichiers/dossiers cachés refusés.
        assert!(resolve_path(&root, "/.git/config").is_none());
        assert!(resolve_path(&root, "/sub/.env").is_none());
    }

    #[test]
    fn resolve_path_allows_normal() {
        let root = std::env::current_dir().unwrap();
        let p = resolve_path(&root, "/src/main.rs").expect("chemin normal accepté");
        assert!(p.ends_with("src/main.rs"));
    }

    #[test]
    fn sanitize_filename_strips_path() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("C:\\Windows\\evil.exe"), "evil.exe");
        assert_eq!(sanitize_filename("simple.txt"), "simple.txt");
        assert_eq!(sanitize_filename(".."), "");
    }

    #[test]
    fn parse_range_variants() {
        let mut h = HeaderMap::new();
        h.insert(header::RANGE, "bytes=0-99".parse().unwrap());
        assert!(matches!(parse_range(&h, 1000), RangeResult::Partial(0, 99)));

        h.insert(header::RANGE, "bytes=100-".parse().unwrap());
        assert!(matches!(parse_range(&h, 1000), RangeResult::Partial(100, 999)));

        h.insert(header::RANGE, "bytes=-50".parse().unwrap());
        assert!(matches!(parse_range(&h, 1000), RangeResult::Partial(950, 999)));

        h.insert(header::RANGE, "bytes=2000-3000".parse().unwrap());
        assert!(matches!(parse_range(&h, 1000), RangeResult::Unsatisfiable));
    }
}
