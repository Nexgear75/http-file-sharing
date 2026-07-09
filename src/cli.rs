//! Définition des arguments CLI (clap derive) et affichage du bandeau terminal
//! de lancement, incluant la section « Depuis un autre appareil » avec QR code.

use std::net::IpAddr;
use std::path::PathBuf;

use owo_colors::OwoColorize;
use qrcode::render::unicode;
use qrcode::QrCode;

/// Serveur HTTP de fichiers ultra-rapide — un remplaçant moderne de
/// `python -m http.server`, avec une interface web épurée et un affichage
/// terminal riche.
#[derive(Debug, Clone, clap::Parser)]
#[command(
    name = "serve",
    version,
    about = "📁 Serve — Serveur HTTP de fichiers moderne en Rust 🦀",
    long_about = None,
    args_conflicts_with_subcommands = true,
    subcommand_negates_reqs = true
)]
pub struct Args {
    /// Sous-commande optionnelle (ex. `serve update`). Sans sous-commande, le
    /// serveur démarre normalement.
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Répertoire à servir.
    #[arg(default_value = ".")]
    pub directory: PathBuf,

    /// Port d'écoute.
    #[arg(short, long, default_value_t = 8080)]
    pub port: u16,

    /// Adresse de bind.
    #[arg(short, long, default_value = "0.0.0.0")]
    pub bind: String,

    /// Ouvrir automatiquement le navigateur au lancement (désactivé par défaut).
    #[arg(long)]
    pub open: bool,

    /// Désactiver le listing de répertoire (404 sur les dossiers sans index.html).
    #[arg(long)]
    pub no_index: bool,

    /// Activer l'upload de fichiers via l'interface web.
    #[arg(long)]
    pub upload: bool,

    /// Protection basique (Basic Auth), au format `USER:PASS`.
    #[arg(long, value_name = "USER:PASS")]
    pub auth: Option<String>,

    /// Activer les headers CORS (Access-Control-Allow-Origin: *).
    #[arg(long)]
    pub cors: bool,

    /// Certificat TLS (HTTPS). [différé]
    #[arg(long, value_name = "FILE")]
    pub tls_cert: Option<PathBuf>,

    /// Clé privée TLS (HTTPS). [différé]
    #[arg(long, value_name = "FILE")]
    pub tls_key: Option<PathBuf>,

    /// Mode compact pour l'affichage terminal (moins verbeux).
    #[arg(long)]
    pub compact: bool,

    /// Taille maximale d'upload en Mo.
    #[arg(long, default_value_t = 100)]
    pub max_upload_mb: u64,

    /// Désactiver la vérification de mise à jour au démarrage.
    #[arg(long)]
    pub no_update_check: bool,
}

/// Sous-commandes de `serve`.
#[derive(Debug, Clone, clap::Subcommand)]
pub enum Command {
    /// Mettre à jour serve vers la dernière version publiée sur GitHub.
    Update {
        /// Vérifier seulement la disponibilité d'une mise à jour, sans l'installer.
        #[arg(long)]
        check: bool,
    },
}

impl Args {
    /// Identifiants Basic Auth parsés depuis `--auth USER:PASS`, s'ils existent.
    pub fn auth_credentials(&self) -> Option<(String, String)> {
        self.auth.as_ref().and_then(|s| {
            s.split_once(':')
                .map(|(u, p)| (u.to_string(), p.to_string()))
        })
    }
}

/// Informations calculées au lancement pour l'affichage du bandeau.
pub struct BannerInfo<'a> {
    pub version: &'a str,
    pub scheme: &'a str,
    pub port: u16,
    pub local_url: String,
    pub network_ips: &'a [IpAddr],
    pub serving_dir: &'a str,
    pub file_count: usize,
    pub dir_count: usize,
    pub total_size: u64,
    pub upload: bool,
    pub auth: bool,
    pub cors: bool,
}

/// Affiche le bandeau de lancement coloré, avec la section « Depuis un autre
/// appareil » très visible (URLs réseau + QR code ASCII).
pub fn print_banner(info: &BannerInfo<'_>) {
    let title = format!("📁 Serve v{} — HTTP File Server", info.version);
    println!();
    println!("{}", "══════════════════════════════════════════════════════════".bright_cyan());
    println!("  {}", title.bold().bright_white());
    println!("{}", "══════════════════════════════════════════════════════════".bright_cyan());
    println!();
    println!("  🖥  {}   {}", "Local :".bold(), info.local_url.bright_green().underline());
    println!();

    print_remote_section(info);

    println!();
    println!("  📂 {}  {}", "Serving :".bold(), info.serving_dir.bright_white());
    println!(
        "  📊 {}  {} fichiers · {} dossiers · {}",
        "Files   :".bold(),
        info.file_count.to_string().bright_yellow(),
        info.dir_count.to_string().bright_yellow(),
        crate::listing::format_size(info.total_size).bright_yellow(),
    );

    // Options actives.
    let mut opts: Vec<String> = Vec::new();
    if info.upload {
        opts.push("upload".to_string());
    }
    if info.auth {
        opts.push("auth".to_string());
    }
    if info.cors {
        opts.push("cors".to_string());
    }
    if !opts.is_empty() {
        println!("  ⚙  {}  {}", "Options :".bold(), opts.join(", ").bright_magenta());
    }

    println!();
    println!("{}", "── Connexions ────────────────────────────────────────────".bright_cyan());
    println!("{}", "  (Ctrl+C pour arrêter proprement)".dimmed());
    println!();
}

/// Affiche la section encadrée « 📡 Depuis un autre appareil ».
fn print_remote_section(info: &BannerInfo<'_>) {
    let border = "─".repeat(56);
    println!("  {}", format!("┌─ 📡 Depuis un autre appareil {}┐", "─".repeat(26)).bright_yellow());
    println!(
        "  {}  {}",
        "│".bright_yellow(),
        "Ouvre ce lien sur l'autre machine ou scanne le QR :".bright_white()
    );
    println!("  {}", format!("│{}│", " ".repeat(56)).bright_yellow());

    if info.network_ips.is_empty() {
        println!(
            "  {}  {}",
            "│".bright_yellow(),
            "(aucune interface réseau détectée — utilise localhost)".dimmed()
        );
    } else {
        for ip in info.network_ips {
            let url = format!("{}://{}:{}", info.scheme, ip, info.port);
            println!("  {}  ▸  {}", "│".bright_yellow(), url.bright_green().bold());
        }
    }
    println!("  {}", format!("│{}│", " ".repeat(56)).bright_yellow());

    // QR code pour la première IP réseau (ou localhost en dernier recours).
    let qr_target = info
        .network_ips
        .first()
        .map(|ip| format!("{}://{}:{}", info.scheme, ip, info.port))
        .unwrap_or_else(|| info.local_url.clone());

    match render_qr(&qr_target) {
        Ok(qr) => {
            for line in qr.lines() {
                println!("  {}   {}", "│".bright_yellow(), line);
            }
        }
        Err(_) => {
            println!(
                "  {}  {}",
                "│".bright_yellow(),
                "(QR code indisponible)".dimmed()
            );
        }
    }
    println!("  {}", format!("│  {}  scanne pour ouvrir{}│", "▲".dimmed(), " ".repeat(33)).bright_yellow());
    println!("  {}", format!("└{}┘", "─".repeat(56)).bright_yellow());
    let _ = border;
}

/// Génère un QR code ASCII (blocs unicode) pour l'URL donnée.
fn render_qr(url: &str) -> Result<String, qrcode::types::QrError> {
    let code = QrCode::new(url.as_bytes())?;
    let image = code
        .render::<unicode::Dense1x2>()
        .dark_color(unicode::Dense1x2::Light)
        .light_color(unicode::Dense1x2::Dark)
        .quiet_zone(true)
        .build();
    Ok(image)
}
