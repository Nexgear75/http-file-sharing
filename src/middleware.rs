//! Middlewares : Basic Auth et rate limiting (token bucket par IP).
//!
//! La compression (gzip/brotli) et le CORS sont fournis par des couches
//! `tower-http` assemblées dans `server.rs`.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::Instant;

use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::{header, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine;

use crate::error::error_response;
use crate::state::AppState;

/// Limiteur de débit par IP, basé sur un token bucket.
pub struct RateLimiter {
    buckets: Mutex<HashMap<IpAddr, Bucket>>,
    /// Nombre maximal de jetons (rafale).
    capacity: f64,
    /// Jetons rechargés par seconde.
    refill_per_sec: f64,
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

impl RateLimiter {
    /// Crée un limiteur ~`rate` req/s avec une rafale égale à `rate`.
    pub fn new(rate: f64) -> Self {
        Self {
            buckets: Mutex::new(HashMap::new()),
            capacity: rate,
            refill_per_sec: rate,
        }
    }

    /// Retourne `true` si la requête est autorisée, `false` si l'IP dépasse le
    /// quota.
    pub fn check(&self, ip: IpAddr) -> bool {
        let now = Instant::now();
        let mut guard = match self.buckets.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let bucket = guard.entry(ip).or_insert(Bucket {
            tokens: self.capacity,
            last: now,
        });
        let elapsed = now.duration_since(bucket.last).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * self.refill_per_sec).min(self.capacity);
        bucket.last = now;

        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Middleware de rate limiting.
pub async fn rate_limit(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<std::net::SocketAddr>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if state.rate_limiter.check(addr.ip()) {
        next.run(req).await
    } else {
        error_response(StatusCode::TOO_MANY_REQUESTS)
    }
}

/// Middleware Basic Auth. Ne fait rien si `--auth` n'est pas configuré.
pub async fn basic_auth(
    State(state): State<AppState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let Some((user, pass)) = &state.config.auth else {
        return next.run(req).await;
    };

    let provided = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(parse_basic_auth);

    match provided {
        Some((u, p)) if &u == user && &p == pass => next.run(req).await,
        _ => (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Basic realm=\"Serve\"")],
        )
            .into_response(),
    }
}

/// Décode un header `Authorization: Basic <base64(user:pass)>`.
fn parse_basic_auth(header_value: &str) -> Option<(String, String)> {
    let encoded = header_value.strip_prefix("Basic ")?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let (u, p) = text.split_once(':')?;
    Some((u.to_string(), p.to_string()))
}
