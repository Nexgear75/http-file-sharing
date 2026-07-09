//! Pages d'erreur HTML stylées et helpers de réponse d'erreur.

use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Response};

/// Construit une réponse d'erreur HTML stylée pour un code de statut donné.
pub fn error_response(status: StatusCode) -> Response {
    let (title, message, emoji) = match status {
        StatusCode::NOT_FOUND => ("404", "Cette ressource est introuvable.", "🔍"),
        StatusCode::FORBIDDEN => ("403", "Accès refusé.", "🚫"),
        StatusCode::UNAUTHORIZED => ("401", "Authentification requise.", "🔒"),
        StatusCode::PAYLOAD_TOO_LARGE => ("413", "Fichier trop volumineux.", "📦"),
        StatusCode::TOO_MANY_REQUESTS => ("429", "Trop de requêtes, réessaie plus tard.", "⏳"),
        StatusCode::BAD_REQUEST => ("400", "Requête invalide.", "⚠️"),
        _ => ("500", "Une erreur interne est survenue.", "💥"),
    };

    let body = format!(
        r#"<!DOCTYPE html>
<html lang="fr">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>{title} — Serve</title>
<style>
  :root {{ color-scheme: dark; }}
  * {{ margin: 0; padding: 0; box-sizing: border-box; }}
  body {{
    font-family: system-ui, -apple-system, "Segoe UI", Inter, sans-serif;
    background: #0d1117; color: #e6edf3;
    min-height: 100vh; display: grid; place-items: center; text-align: center;
    padding: 2rem;
  }}
  .card {{ max-width: 30rem; }}
  .emoji {{ font-size: 4rem; }}
  h1 {{
    font-size: 5rem; font-weight: 800; letter-spacing: -0.05em;
    background: linear-gradient(135deg, #58a6ff, #bc8cff);
    -webkit-background-clip: text; background-clip: text; color: transparent;
    margin: 0.5rem 0;
  }}
  p {{ color: #8b949e; font-size: 1.1rem; margin-bottom: 2rem; }}
  a {{
    display: inline-block; padding: 0.6rem 1.4rem; border-radius: 0.6rem;
    background: #238636; color: #fff; text-decoration: none; font-weight: 600;
    transition: background 0.2s ease;
  }}
  a:hover {{ background: #2ea043; }}
</style>
</head>
<body>
  <div class="card">
    <div class="emoji">{emoji}</div>
    <h1>{title}</h1>
    <p>{message}</p>
    <a href="/">← Retour à l'accueil</a>
  </div>
</body>
</html>"#
    );

    (
        status,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        Html(body),
    )
        .into_response()
}
