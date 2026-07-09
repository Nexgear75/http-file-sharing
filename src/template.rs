//! Rendu du template HTML front-end embarqué dans le binaire.

use crate::listing::Entry;

/// Template HTML embarqué à la compilation.
const TEMPLATE: &str = include_str!("assets/listing.html");

/// Un fragment de breadcrumb (nom affiché + URL cible).
pub struct Crumb {
    pub name: String,
    pub url: String,
}

/// Rend la page de listing en remplaçant les placeholders `{{...}}`.
pub fn render(
    dir_name: &str,
    crumbs: &[Crumb],
    entries: &[Entry],
    has_upload: bool,
    version: &str,
) -> String {
    let files_json = serde_json::to_string(entries).unwrap_or_else(|_| "[]".to_string());
    let breadcrumb_html = render_breadcrumb(crumbs);

    TEMPLATE
        .replace("{{DIRECTORY}}", &html_escape(dir_name))
        .replace("{{BREADCRUMB}}", &breadcrumb_html)
        .replace("{{FILES_JSON}}", &files_json)
        .replace("{{HAS_UPLOAD}}", if has_upload { "true" } else { "false" })
        .replace("{{SERVER_VERSION}}", &html_escape(version))
}

/// Construit le HTML du fil d'Ariane cliquable.
fn render_breadcrumb(crumbs: &[Crumb]) -> String {
    let mut html = String::new();
    for (i, crumb) in crumbs.iter().enumerate() {
        if i > 0 {
            html.push_str("<span class=\"sep\">/</span>");
        }
        html.push_str(&format!(
            "<a href=\"{}\" class=\"crumb\">{}</a>",
            html_escape(&crumb.url),
            html_escape(&crumb.name)
        ));
    }
    html
}

/// Échappement HTML minimal.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_replaces_all_placeholders() {
        let html = render("mondossier", &[], &[], true, "1.0.0");
        assert!(!html.contains("{{DIRECTORY}}"));
        assert!(!html.contains("{{BREADCRUMB}}"));
        assert!(!html.contains("{{FILES_JSON}}"));
        assert!(!html.contains("{{HAS_UPLOAD}}"));
        assert!(!html.contains("{{SERVER_VERSION}}"));
        assert!(html.contains("mondossier"));
        assert!(html.contains("const FILES = []"));
        assert!(html.contains("const HAS_UPLOAD = true"));
    }

    #[test]
    fn html_escape_prevents_injection() {
        assert_eq!(html_escape("<script>"), "&lt;script&gt;");
        assert_eq!(html_escape("a & b"), "a &amp; b");
    }
}
