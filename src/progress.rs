//! Barres de progression console (via `indicatif`) et statistiques globales.
//!
//! - Chaque transfert (download `↓` ou upload `↑`) obtient une barre dédiée,
//!   mise à jour à chaque chunk et remplacée par une ligne de résumé à la fin.
//! - Une barre globale s'affiche dès que ≥ 2 transferts sont actifs.
//! - Les requêtes « rapides » (listing, 404, petits fichiers) sont loguées sur
//!   une seule ligne.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use owo_colors::OwoColorize;

use crate::listing::format_size;

/// Sens d'un transfert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Le client télécharge depuis le serveur.
    Download,
    /// Le client envoie vers le serveur.
    Upload,
}

impl Direction {
    fn arrow(self) -> &'static str {
        match self {
            Direction::Download => "↓",
            Direction::Upload => "↑",
        }
    }
}

/// Un transfert actif tel que suivi pour la barre globale.
struct ActiveTransfer {
    current: u64,
    total: u64,
}

struct Inner {
    active: HashMap<u64, ActiveTransfer>,
    global: Option<ProgressBar>,
    total_requests: u64,
    total_bytes: u64,
}

/// Suivi global de la progression et du logging console.
pub struct ProgressTracker {
    mp: MultiProgress,
    compact: bool,
    inner: Mutex<Inner>,
    next_id: AtomicU64,
    start: Instant,
}

impl ProgressTracker {
    pub fn new(compact: bool) -> Self {
        let mp = MultiProgress::with_draw_target(ProgressDrawTarget::stdout());
        Self {
            mp,
            compact,
            inner: Mutex::new(Inner {
                active: HashMap::new(),
                global: None,
                total_requests: 0,
                total_bytes: 0,
            }),
            next_id: AtomicU64::new(1),
            start: Instant::now(),
        }
    }

    /// Style d'une barre de transfert active.
    fn transfer_style(&self) -> ProgressStyle {
        let tmpl = if self.compact {
            "  [{percent:>3}%] {prefix}"
        } else {
            "  {prefix} [{bar:20.cyan/blue}] {percent:>3}% {bytes}/{total_bytes} {binary_bytes_per_sec} {elapsed}"
        };
        ProgressStyle::with_template(tmpl)
            .unwrap_or_else(|_| ProgressStyle::default_bar())
            .progress_chars("█░")
    }

    /// Démarre le suivi d'un transfert et retourne un handle à alimenter.
    pub fn start_transfer(
        self: &Arc<Self>,
        direction: Direction,
        method: &str,
        path: &str,
        ip: String,
        total: u64,
    ) -> TransferHandle {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let prefix = format!("{} {:<4} {}  {}", direction.arrow(), method, path, ip);

        let mut guard = match self.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };

        // Insère la barre juste au-dessus de la barre globale si elle existe.
        let pb = match &guard.global {
            Some(g) => self.mp.insert_before(g, ProgressBar::new(total.max(1))),
            None => self.mp.add(ProgressBar::new(total.max(1))),
        };
        pb.set_style(self.transfer_style());
        pb.set_prefix(prefix);

        guard.active.insert(id, ActiveTransfer { current: 0, total });
        self.refresh_global(&mut guard);

        TransferHandle {
            tracker: Arc::clone(self),
            id,
            pb,
            direction,
            method: method.to_string(),
            path: path.to_string(),
            ip,
            total,
            current: AtomicU64::new(0),
            finished: AtomicBool::new(false),
        }
    }

    /// Logue une requête « rapide » (sans barre) sur une seule ligne.
    pub fn log_request(
        &self,
        method: &str,
        path: &str,
        ip: &str,
        status: u16,
        duration: Duration,
        bytes: u64,
    ) {
        {
            let mut guard = match self.inner.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            guard.total_requests += 1;
            guard.total_bytes += bytes;
        }

        let ok = (200..400).contains(&status);
        let mark = if ok {
            "✓".green().to_string()
        } else {
            "✗".red().to_string()
        };
        let status_str = if ok {
            status.green().to_string()
        } else {
            status.red().to_string()
        };
        let line = format!(
            "{mark} {:<5}{:<28} → {status_str}  {:>7.1}ms  {ip}",
            method,
            truncate(path, 28),
            duration.as_secs_f64() * 1000.0,
        );
        let _ = self.mp.println(line);
    }

    /// Recalcule / affiche / masque la barre globale selon les transferts actifs.
    fn refresh_global(&self, guard: &mut Inner) {
        let count = guard.active.len();
        if count >= 2 {
            let total: u64 = guard.active.values().map(|a| a.total).sum();
            let current: u64 = guard.active.values().map(|a| a.current).sum();

            let bar = guard.global.get_or_insert_with(|| {
                let pb = self.mp.add(ProgressBar::new(total.max(1)));
                let style = ProgressStyle::with_template(
                    "  {prefix} [{bar:24.green/black}] {percent:>3}% {bytes}/{total_bytes}",
                )
                .unwrap_or_else(|_| ProgressStyle::default_bar())
                .progress_chars("█░");
                pb.set_style(style);
                pb
            });
            bar.set_length(total.max(1));
            bar.set_position(current);
            bar.set_prefix(format!("══ Transferts : {count} actifs"));
        } else if let Some(bar) = guard.global.take() {
            bar.finish_and_clear();
        }
    }

    /// Affiche un résumé final à la fermeture du serveur.
    pub fn print_summary(&self) {
        let guard = match self.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let elapsed = self.start.elapsed();
        println!();
        println!(
            "{}",
            format!(
                "── Résumé ── {} requêtes · {} transférés · uptime {}",
                guard.total_requests,
                format_size(guard.total_bytes),
                format_duration(elapsed),
            )
            .bright_cyan()
        );
    }
}

/// Handle d'un transfert en cours ; à alimenter via [`TransferHandle::inc`] puis
/// à clore via [`TransferHandle::finish`]. En cas d'abandon (Drop sans finish),
/// le transfert est retiré proprement.
pub struct TransferHandle {
    tracker: Arc<ProgressTracker>,
    id: u64,
    pb: ProgressBar,
    direction: Direction,
    method: String,
    path: String,
    ip: String,
    total: u64,
    current: AtomicU64,
    finished: AtomicBool,
}

impl TransferHandle {
    /// Incrémente la progression de `n` octets.
    pub fn inc(&self, n: u64) {
        let current = self.current.fetch_add(n, Ordering::Relaxed) + n;
        self.pb.set_position(current);

        let mut guard = match self.tracker.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if let Some(active) = guard.active.get_mut(&self.id) {
            active.current = current;
        }
        self.tracker.refresh_global(&mut guard);
    }

    /// Clôt le transfert avec succès et affiche une ligne de résumé.
    pub fn finish(&self, status: u16) {
        if self.finished.swap(true, Ordering::SeqCst) {
            return;
        }
        let transferred = self.current.load(Ordering::Relaxed);
        let elapsed = self.pb.elapsed();
        let speed = if elapsed.as_secs_f64() > 0.0 {
            transferred as f64 / elapsed.as_secs_f64()
        } else {
            transferred as f64
        };

        self.pb.finish_and_clear();

        let mut guard = match self.tracker.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        guard.active.remove(&self.id);
        guard.total_requests += 1;
        guard.total_bytes += transferred;
        self.tracker.refresh_global(&mut guard);
        drop(guard);

        let ok = (200..400).contains(&status);
        let mark = if ok {
            "✓".green().to_string()
        } else {
            "✗".red().to_string()
        };
        let line = format!(
            "{mark} {:<5}{:<28} {} {}  → {}  {}  {}  ({}/s)",
            self.method,
            truncate(&self.path, 28),
            self.direction.arrow(),
            self.ip,
            status,
            format_size(transferred),
            format_duration(elapsed),
            format_size(speed as u64),
        );
        let _ = self.tracker.mp.println(line);
    }
}

impl Drop for TransferHandle {
    fn drop(&mut self) {
        // Transfert interrompu (déconnexion client) : nettoyage silencieux.
        if self.finished.swap(true, Ordering::SeqCst) {
            return;
        }
        self.pb.finish_and_clear();
        let mut guard = match self.tracker.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        guard.active.remove(&self.id);
        self.tracker.refresh_global(&mut guard);
        drop(guard);

        let transferred = self.current.load(Ordering::Relaxed);
        let line = format!(
            "{} {:<5}{:<28} {} {}  → interrompu ({} transférés)",
            "⚠".yellow(),
            self.method,
            truncate(&self.path, 28),
            self.direction.arrow(),
            self.ip,
            format_size(transferred),
        );
        let _ = self.tracker.mp.println(line);
        let _ = self.total;
    }
}

/// Tronque une chaîne à `max` caractères (avec `…` si coupée).
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// Formate une durée de façon compacte (ex: `0.8s`, `1m03s`).
fn format_duration(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 60.0 {
        format!("{secs:.1}s")
    } else {
        let m = (secs / 60.0).floor() as u64;
        let s = (secs - (m * 60) as f64).floor() as u64;
        format!("{m}m{s:02}s")
    }
}
