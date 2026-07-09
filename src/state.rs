//! État partagé de l'application, injecté dans chaque handler axum.

use std::path::PathBuf;
use std::sync::Arc;

use crate::middleware::RateLimiter;
use crate::progress::ProgressTracker;

/// Configuration résolue au lancement, immuable pendant l'exécution.
#[derive(Debug, Clone)]
pub struct Config {
    /// Racine servie, canonicalisée (chemin absolu).
    pub root: PathBuf,
    pub no_index: bool,
    pub upload: bool,
    pub cors: bool,
    /// Identifiants Basic Auth (user, pass) si `--auth`.
    pub auth: Option<(String, String)>,
    /// Taille maximale d'un upload, en octets.
    pub max_upload_bytes: u64,
    pub version: String,
}

/// État applicatif partagé (Clone bon marché : tout est derrière des `Arc`).
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub tracker: Arc<ProgressTracker>,
    pub rate_limiter: Arc<RateLimiter>,
}
