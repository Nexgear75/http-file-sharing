# 📁 Serve

Serveur HTTP de fichiers ultra-rapide en Rust — un remplaçant moderne de
`python -m http.server`, avec une interface web épurée pour les visiteurs et un
affichage terminal riche pour le serveur.

## Installation

```bash
cargo build --release
# binaire : ./target/release/serve  (~2.2 Mo, sans dépendance runtime)
```

## Utilisation

```bash
serve [OPTIONS] [DIRECTORY]
```

| Option | Description |
|---|---|
| `-p, --port <PORT>` | Port d'écoute (défaut : 8080) |
| `-b, --bind <ADDR>` | Adresse de bind (défaut : 0.0.0.0) |
| `--open` | Ouvrir le navigateur au lancement (désactivé par défaut) |
| `--no-index` | Désactiver le listing (404 sur dossiers sans index.html) |
| `--upload` | Activer l'upload de fichiers via l'interface web |
| `--auth <USER:PASS>` | Protection Basic Auth |
| `--cors` | Activer les headers CORS (`*`) |
| `--tls-cert / --tls-key` | HTTPS *(différé — avertissement au lancement)* |
| `--compact` | Affichage terminal moins verbeux |
| `--max-upload-mb <N>` | Taille max d'upload (défaut : 100 Mo) |

Exemple :

```bash
serve --upload -p 9000 ~/Documents
```

Au lancement, le terminal affiche un bandeau coloré, les URLs réseau pour les
autres appareils et un **QR code** à scanner.

## Fonctionnalités

**Serveur** : fichiers statiques (Content-Type, ETag, Last-Modified, 304),
requêtes `Range` (streaming vidéo/audio), compression gzip/brotli à la volée,
listing élégant, upload multipart streamé, téléchargement de dossier en **zip à
la volée** (sans écriture disque), Basic Auth, CORS, rate limiting par IP
(token bucket ~100 req/s), graceful shutdown sur Ctrl+C.

**Sécurité** : protection contre le path traversal (canonicalisation +
vérification racine), fichiers cachés (`.git`, `.env`…) jamais servis, taille
d'upload plafonnée, aucun `.unwrap()` sur les entrées réseau.

**Terminal** : bandeau + QR code, barres de progression `indicatif` en temps
réel par transfert (↓/↑), barre globale quand ≥ 2 transferts, résumé de
fermeture.

**Interface web** (embarquée dans le binaire, < 30 Ko) : dark/light mode,
recherche instantanée, tri par colonne, breadcrumb, lightbox images avec
navigation, lecteur vidéo/audio/PDF, visionneuse code, toasts de téléchargement
avec barre de progression (`fetch` + `ReadableStream`), modal d'upload
drag & drop avec progression par fichier (`XMLHttpRequest.upload.onprogress`).

## Architecture

```
src/
├── main.rs        Point d'entrée, runtime tokio
├── cli.rs         Arguments clap + bandeau terminal + QR code
├── server.rs      Routeur axum, middlewares, graceful shutdown
├── handler.rs     Routage : fichiers (Range), listing, upload, zip
├── progress.rs    Barres indicatif, TransferHandle, stats globales
├── middleware.rs  Basic Auth, rate limiting (token bucket)
├── template.rs    Rendu du template HTML embarqué
├── listing.rs     Lecture de répertoire → JSON, catégories, tailles
├── network.rs     Détection des IP locales
├── error.rs       Pages d'erreur HTML stylées (404/403/500…)
└── assets/
    └── listing.html   Front-end complet (HTML + CSS + JS inline)
```

Stack : **tokio** · **axum** (hyper) · **tower-http** (compression, CORS) ·
**indicatif** · **async_zip** · **qrcode** · **clap**.

## Multi-plateforme

Le binaire est **portable macOS / Linux / Windows** — aucune dépendance
runtime, tout est statiquement embarqué. Les rares parties spécifiques à l'OS
(ouverture du navigateur via `open` / `xdg-open` / `start`, gestion de SIGTERM)
sont gérées par compilation conditionnelle et vérifiées sur les trois cibles.

Compilation croisée (exemple depuis macOS) :

```bash
rustup target add x86_64-unknown-linux-gnu
cargo build --release --target x86_64-unknown-linux-gnu

rustup target add x86_64-pc-windows-msvc
cargo build --release --target x86_64-pc-windows-msvc
```

> Une compilation *native* (ou via GitHub Actions par OS) reste recommandée pour
> le link final : `cargo check --target …` valide le code sur toutes les cibles,
> mais le link cross nécessite le linker de la plateforme visée.

## Tests

```bash
cargo test
```

Couvre le formatage des tailles, la catégorisation, le tri du listing, le rendu
du template, l'échappement HTML, la protection path traversal et le parsing des
requêtes Range.

## Notes

- HTTPS/TLS est prévu (flags présents) mais son implémentation est différée ;
  un avertissement est affiché et le serveur démarre en HTTP.
- Profil release : `lto`, `codegen-units = 1`, `opt-level = "z"`, `strip`.
