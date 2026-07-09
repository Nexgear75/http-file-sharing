//! Auto-mise à jour du binaire depuis les releases GitHub (crate `self_update`).
//!
//! - `serve update` télécharge et installe la dernière version (avec confirmation).
//! - `serve update --check` indique seulement si une mise à jour existe.
//! - Un check discret en tâche de fond au démarrage signale une version plus
//!   récente sans jamais rien installer.

use anyhow::{Context, Result};

/// Propriétaire du dépôt GitHub hébergeant les releases.
const REPO_OWNER: &str = "Nexgear75";
/// Nom du dépôt GitHub.
const REPO_NAME: &str = "http-file-sharing";
/// Nom du binaire dans les archives de release.
const BIN_NAME: &str = "serve";

/// Version actuellement compilée.
fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Exécute la mise à jour (ou la simple vérification si `check_only`).
pub fn run_update(check_only: bool) -> Result<()> {
    let current = current_version();
    let updater = self_update::backends::github::Update::configure()
        .repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .bin_name(BIN_NAME)
        .current_version(current)
        .show_download_progress(true)
        .build()
        .context("configuration de la mise à jour")?;

    if check_only {
        let latest = updater
            .get_latest_release()
            .context("impossible de récupérer la dernière release depuis GitHub")?;
        if self_update::version::bump_is_greater(current, &latest.version).unwrap_or(false) {
            println!(
                "🔔 Nouvelle version disponible : v{} (actuelle : v{current})",
                latest.version
            );
            println!("   Lance `serve update` pour l'installer.");
        } else {
            println!("✓ serve est à jour (v{current}).");
        }
        return Ok(());
    }

    println!("Recherche de mises à jour sur GitHub…");
    let status = updater.update().context("échec de la mise à jour")?;
    if status.updated() {
        println!("✓ Mis à jour : v{current} → v{}", status.version());
        println!("  Relance `serve` pour utiliser la nouvelle version.");
    } else {
        println!("✓ Déjà à jour (v{current}).");
    }
    Ok(())
}

/// Vérifie en tâche de fond s'il existe une version plus récente.
///
/// Retourne `Some(version)` si une mise à jour est disponible, `None` sinon ou
/// en cas d'erreur (hors ligne, rate limit…) — volontairement silencieux pour
/// ne jamais gêner l'utilisation normale du serveur.
pub fn check_newer() -> Option<String> {
    let current = current_version();
    let updater = self_update::backends::github::Update::configure()
        .repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .bin_name(BIN_NAME)
        .current_version(current)
        .build()
        .ok()?;
    let latest = updater.get_latest_release().ok()?;
    match self_update::version::bump_is_greater(current, &latest.version) {
        Ok(true) => Some(latest.version),
        _ => None,
    }
}
