use axum::{
    extract::Path,
    response::{Html, IntoResponse},
};

/// Statische Pager-Seite (Audio-only, für Durchsagen)
const PAGER_HTML: &str = include_str!("index.html");
const PAGER_JS: &str = include_str!("pager.js");
const PAGER_CSS: &str = include_str!("pager.css");

/// GET /pager/:id – liefert die Pager-Seite
pub async fn pager_page(Path(id): Path<String>) -> impl IntoResponse {
    tracing::info!("Pager-Page aufgerufen für id={id}");
    Html(super::page_text("pager", "index.html", PAGER_HTML))
}

/// GET /pager/pager.js
pub async fn pager_js() -> impl IntoResponse {
    (
        [("Content-Type", "application/javascript; charset=utf-8")],
        super::page_text("pager", "pager.js", PAGER_JS),
    )
}

/// GET /pager/pager.css
pub async fn pager_css() -> impl IntoResponse {
    ([("Content-Type", "text/css; charset=utf-8")], super::page_text("pager", "pager.css", PAGER_CSS))
}
