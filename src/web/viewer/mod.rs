use axum::{
    extract::Path,
    response::{Html, IntoResponse},
};

/// Statisches HTML für den Viewer (gleich für alle IDs)
const VIEWER_HTML: &str = include_str!("index.html");
const VIEWER_JS: &str = include_str!("viewer.js");
const VIEWER_CSS: &str = include_str!("viewer.css");
const VIEWER_BG: &[u8] = include_bytes!("bg_low_poly_alone.png");
const VIEWER_FAVICON: &[u8] = include_bytes!("favicon.ico");

/// GET /viewer/:id  -> liefert die Viewer-Seite
pub async fn viewer_page(Path(id): Path<i64>) -> impl IntoResponse {
    // id brauchst du aktuell nur für das JS (es holt sich die ID aus der URL),
    // aber du kannst sie hier loggen, falls du möchtest:
    tracing::info!("Viewer-Page aufgerufen für id={id}");

    Html(super::page_text("viewer", "index.html", VIEWER_HTML))
}

/// GET /viewer/viewer.js
pub async fn viewer_js() -> impl IntoResponse {
    (
        [("Content-Type", "application/javascript; charset=utf-8")],
        super::page_text("viewer", "viewer.js", VIEWER_JS),
    )
}

/// GET /viewer/viewer.css
pub async fn viewer_css() -> impl IntoResponse {
    ([("Content-Type", "text/css; charset=utf-8")], super::page_text("viewer", "viewer.css", VIEWER_CSS))
}

/// GET /viewer/bg_low_poly_alone.png
pub async fn viewer_bg() -> impl IntoResponse {
    ([("Content-Type", "image/png")], VIEWER_BG)
}

/// GET /viewer/favicon.ico
pub async fn viewer_favicon() -> impl IntoResponse {
    ([("Content-Type", "image/x-icon")], VIEWER_FAVICON)
}
