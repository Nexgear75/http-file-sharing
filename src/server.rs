//! Configuration et lancement du serveur HTTP (axum + tokio), assemblage des
//! middlewares, affichage du bandeau et graceful shutdown.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use axum::Router;
use owo_colors::OwoColorize;
use tower_http::compression::predicate::{DefaultPredicate, NotForContentType, Predicate};
use tower_http::compression::CompressionLayer;
use tower_http::cors::CorsLayer;

use crate::cli::{self, Args, BannerInfo};
use crate::handler;
use crate::listing;
use crate::middleware;
use crate::network;
use crate::progress::ProgressTracker;
use crate::state::{AppState, Config};

/// Point d'entrée async : prépare l'état, affiche le bandeau et sert jusqu'à
/// Ctrl+C.
pub async fn run(args: Args) -> Result<()> {
    let version = env!("CARGO_PKG_VERSION").to_string();

    // Résolution de la racine servie.
    let root = args
        .directory
        .canonicalize()
        .with_context(|| format!("répertoire introuvable : {}", args.directory.display()))?;
    if !root.is_dir() {
        anyhow::bail!("« {} » n'est pas un répertoire", root.display());
    }

    // Avertissement TLS (implémentation différée).
    if args.tls_cert.is_some() || args.tls_key.is_some() {
        eprintln!(
            "{}",
            "⚠  TLS n'est pas encore implémenté — démarrage en HTTP.".yellow()
        );
    }

    // Statistiques du dossier (pour le bandeau).
    let stats = listing::scan_stats(&root, 6);

    // État partagé.
    let tracker = Arc::new(ProgressTracker::new(args.compact));
    let rate_limiter = Arc::new(middleware::RateLimiter::new(100.0));
    let config = Arc::new(Config {
        root: root.clone(),
        no_index: args.no_index,
        upload: args.upload,
        cors: args.cors,
        auth: args.auth_credentials(),
        max_upload_bytes: args.max_upload_mb.saturating_mul(1024 * 1024),
        version: version.clone(),
    });
    let state = AppState {
        config: config.clone(),
        tracker: tracker.clone(),
        rate_limiter,
    };

    // Adresse d'écoute.
    let bind_addr: SocketAddr = format!("{}:{}", args.bind, args.port)
        .parse()
        .with_context(|| format!("adresse de bind invalide : {}:{}", args.bind, args.port))?;

    let network_ips = network::local_ips();
    let local_url = format!("http://localhost:{}", args.port);

    // Bandeau de lancement.
    let banner = BannerInfo {
        version: &version,
        scheme: "http",
        port: args.port,
        local_url: local_url.clone(),
        network_ips: &network_ips,
        serving_dir: &root.display().to_string(),
        file_count: stats.files,
        dir_count: stats.dirs,
        total_size: stats.total_size,
        upload: args.upload,
        auth: config.auth.is_some(),
        cors: args.cors,
    };
    cli::print_banner(&banner);

    // Ouverture du navigateur uniquement si demandée explicitement (--open).
    if args.open {
        open_browser(&local_url);
    }

    // Vérification de mise à jour discrète, en tâche de fond (non bloquante).
    if !args.no_update_check {
        tokio::spawn(async {
            if let Ok(Some(version)) = tokio::task::spawn_blocking(crate::update::check_newer).await
            {
                println!(
                    "\n{}",
                    format!(
                        "🔔 serve v{version} est disponible — lance `serve update` pour mettre à jour"
                    )
                    .bright_yellow()
                );
            }
        });
    }

    // Construction du routeur.
    let app = build_router(state);

    let listener = tokio::net::TcpListener::bind(bind_addr)
        .await
        .with_context(|| format!("impossible d'écouter sur {bind_addr}"))?;

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .context("erreur du serveur HTTP")?;

    // Résumé de fermeture.
    println!();
    println!("{}", "👋 Fermeture — transferts terminés proprement.".bright_cyan());
    tracker.print_summary();

    Ok(())
}

/// Assemble le routeur avec ses middlewares.
fn build_router(state: AppState) -> Router {
    let cors_enabled = state.config.cors;

    // Compression réservée aux contenus texte : on exclut les types déjà
    // compressés ou incompressibles (binaires, archives, médias). Bénéfice
    // double : on ne gaspille pas de CPU, et surtout le `Content-Length` des
    // téléchargements est préservé (indispensable à la barre de progression
    // côté navigateur).
    let compression = CompressionLayer::new().compress_when(
        DefaultPredicate::new()
            .and(NotForContentType::const_new("application/octet-stream"))
            .and(NotForContentType::const_new("application/zip"))
            .and(NotForContentType::const_new("application/pdf"))
            .and(NotForContentType::const_new("application/x-"))
            .and(NotForContentType::const_new("video/"))
            .and(NotForContentType::const_new("audio/"))
            .and(NotForContentType::const_new("font/")),
    );

    let mut app = Router::new()
        .route("/__health", get(handler::health))
        .route(
            "/__upload",
            post(handler::upload).layer(DefaultBodyLimit::disable()),
        )
        .fallback(handler::serve)
        .layer(compression)
        // Auth puis rate limiting (rate limiting = couche la plus externe).
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::basic_auth,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::rate_limit,
        ));

    if cors_enabled {
        app = app.layer(
            CorsLayer::permissive()
                .expose_headers([axum::http::HeaderName::from_static("x-file-size")]),
        );
    }

    app.with_state(state)
}

/// Ouvre l'URL dans le navigateur par défaut selon la plateforme.
fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let cmd = ("open", vec![url]);
    #[cfg(target_os = "windows")]
    let cmd = ("cmd", vec!["/C", "start", url]);
    #[cfg(all(unix, not(target_os = "macos")))]
    let cmd = ("xdg-open", vec![url]);

    let _ = std::process::Command::new(cmd.0)
        .args(cmd.1)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Attend Ctrl+C (ou SIGTERM sur Unix) pour déclencher le graceful shutdown.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut sig) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            sig.recv().await;
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
