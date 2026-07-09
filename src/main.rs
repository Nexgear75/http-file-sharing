//! `serve` — Serveur HTTP de fichiers ultra-rapide en Rust.
//!
//! Point d'entrée : parse la CLI, affiche le bandeau de lancement puis démarre
//! le serveur HTTP (axum + tokio) avec graceful shutdown sur Ctrl+C.

mod cli;
mod error;
mod handler;
mod listing;
mod middleware;
mod network;
mod progress;
mod server;
mod state;
mod template;

use std::process::ExitCode;

use clap::Parser;

use crate::cli::Args;

fn main() -> ExitCode {
    let args = Args::parse();

    // Runtime tokio multi-thread.
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("Erreur : impossible de démarrer le runtime tokio : {e}");
            return ExitCode::FAILURE;
        }
    };

    match runtime.block_on(server::run(args)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Erreur : {e:#}");
            ExitCode::FAILURE
        }
    }
}
