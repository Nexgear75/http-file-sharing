//! Lecture d'un répertoire et génération des données de listing (sérialisées en
//! JSON pour le front-end). Fournit aussi le formatage lisible des tailles.

use std::path::Path;

use chrono::{DateTime, Utc};
use serde::Serialize;

/// Une entrée de répertoire telle que consommée par le front-end.
#[derive(Debug, Serialize)]
pub struct Entry {
    pub name: String,
    /// Chemin relatif à la racine servie (avec `/` comme séparateur).
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    /// Date de modification au format ISO-8601 (UTC).
    pub modified: Option<String>,
    pub mime_type: String,
    /// Catégorie logique : folder, image, video, audio, code, archive, pdf,
    /// document, font, binary.
    pub category: String,
}

/// Statistiques agrégées d'un dossier.
#[derive(Debug, Clone, Copy, Default)]
pub struct DirStats {
    pub files: usize,
    pub dirs: usize,
    pub total_size: u64,
}

/// Lit les entrées d'un répertoire et retourne la liste triée (dossiers
/// d'abord, puis fichiers, alphabétiquement). Les fichiers cachés (préfixe `.`)
/// sont ignorés.
///
/// `url_prefix` est le chemin URL du dossier courant (ex: `images/`) servant à
/// construire le champ `path` de chaque entrée.
pub async fn read_dir(dir: &Path, url_prefix: &str) -> std::io::Result<Vec<Entry>> {
    let mut entries: Vec<Entry> = Vec::new();
    let mut rd = tokio::fs::read_dir(dir).await?;

    while let Some(dirent) = rd.next_entry().await? {
        let name = dirent.file_name().to_string_lossy().to_string();

        // Sécurité / propreté : on ne liste pas les fichiers cachés.
        if name.starts_with('.') {
            continue;
        }

        let metadata = match dirent.metadata().await {
            Ok(m) => m,
            Err(_) => continue,
        };
        let is_dir = metadata.is_dir();

        let modified = metadata
            .modified()
            .ok()
            .map(|t| DateTime::<Utc>::from(t).to_rfc3339());

        let rel_path = if url_prefix.is_empty() {
            name.clone()
        } else {
            format!("{}/{}", url_prefix.trim_end_matches('/'), name)
        };

        let (mime_type, category) = if is_dir {
            ("inode/directory".to_string(), "folder".to_string())
        } else {
            let mime = mime_guess::from_path(&name)
                .first_or_octet_stream()
                .to_string();
            let cat = categorize(&name, &mime);
            (mime, cat)
        };

        entries.push(Entry {
            name,
            path: rel_path,
            is_dir,
            size: if is_dir { 0 } else { metadata.len() },
            modified,
            mime_type,
            category,
        });
    }

    entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });

    Ok(entries)
}

/// Calcule récursivement (un seul niveau + récursion) les stats d'un dossier :
/// nombre de fichiers, de dossiers et taille totale.
///
/// Utilisé au lancement pour l'affichage terminal. La récursion est bornée en
/// profondeur pour éviter les mauvaises surprises sur d'énormes arborescences.
pub fn scan_stats(dir: &Path, max_depth: usize) -> DirStats {
    let mut stats = DirStats::default();
    scan_stats_inner(dir, max_depth, &mut stats);
    stats
}

fn scan_stats_inner(dir: &Path, depth_left: usize, stats: &mut DirStats) {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => return,
    };
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            stats.dirs += 1;
            if depth_left > 0 {
                scan_stats_inner(&entry.path(), depth_left - 1, stats);
            }
        } else {
            stats.files += 1;
            stats.total_size += meta.len();
        }
    }
}

/// Détermine la catégorie logique d'un fichier à partir de son nom et de son
/// type MIME.
pub fn categorize(name: &str, mime: &str) -> String {
    let ext = name
        .rsplit('.')
        .next()
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    if mime.starts_with("image/") {
        return "image".to_string();
    }
    if mime.starts_with("video/") {
        return "video".to_string();
    }
    if mime.starts_with("audio/") {
        return "audio".to_string();
    }
    if mime == "application/pdf" {
        return "pdf".to_string();
    }
    if mime.starts_with("font/") || matches!(ext.as_str(), "ttf" | "otf" | "woff" | "woff2" | "eot") {
        return "font".to_string();
    }

    match ext.as_str() {
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "7z" | "rar" | "zst" => "archive".to_string(),
        "rs" | "js" | "ts" | "jsx" | "tsx" | "py" | "rb" | "go" | "c" | "h" | "cpp" | "hpp"
        | "cc" | "java" | "kt" | "swift" | "php" | "cs" | "sh" | "bash" | "zsh" | "html"
        | "htm" | "css" | "scss" | "sass" | "json" | "yaml" | "yml" | "toml" | "xml" | "sql"
        | "lua" | "pl" | "r" | "dart" | "vue" | "svelte" => "code".to_string(),
        "txt" | "md" | "markdown" | "rst" | "doc" | "docx" | "odt" | "rtf" | "csv" | "xls"
        | "xlsx" | "ppt" | "pptx" | "epub" => "document".to_string(),
        _ => {
            if mime.starts_with("text/") {
                "code".to_string()
            } else {
                "binary".to_string()
            }
        }
    }
}

/// Formate une taille en octets de façon lisible (o, Ko, Mo, Go, To — base 1024).
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["o", "Ko", "Mo", "Go", "To"];
    if bytes < 1024 {
        return format!("{bytes} o");
    }
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    format!("{size:.1} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_size_units() {
        assert_eq!(format_size(0), "0 o");
        assert_eq!(format_size(512), "512 o");
        assert_eq!(format_size(1024), "1.0 Ko");
        assert_eq!(format_size(1536), "1.5 Ko");
        assert_eq!(format_size(1024 * 1024), "1.0 Mo");
        assert_eq!(format_size(1024 * 1024 * 1024), "1.0 Go");
    }

    #[test]
    fn categorize_by_extension() {
        assert_eq!(categorize("photo.jpg", "image/jpeg"), "image");
        assert_eq!(categorize("clip.mp4", "video/mp4"), "video");
        assert_eq!(categorize("song.mp3", "audio/mpeg"), "audio");
        assert_eq!(categorize("doc.pdf", "application/pdf"), "pdf");
        assert_eq!(categorize("main.rs", "text/plain"), "code");
        assert_eq!(categorize("archive.zip", "application/zip"), "archive");
        assert_eq!(categorize("font.woff2", "font/woff2"), "font");
        assert_eq!(categorize("notes.md", "text/markdown"), "document");
        assert_eq!(categorize("blob.dat", "application/octet-stream"), "binary");
    }

    #[tokio::test]
    async fn read_dir_orders_dirs_first_and_skips_hidden() {
        let tmp = std::env::temp_dir().join(format!("serve-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(tmp.join("zdir"));
        std::fs::write(tmp.join("a.txt"), b"x").unwrap();
        std::fs::write(tmp.join(".hidden"), b"x").unwrap();

        let entries = read_dir(&tmp, "").await.unwrap();
        assert!(entries.iter().all(|e| e.name != ".hidden"));
        // Le dossier vient avant le fichier.
        assert!(entries[0].is_dir);
        assert_eq!(entries[0].name, "zdir");

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
