use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use axum::{extract::Extension, response::Html, routing::get, Router};
use once_cell::sync::OnceCell;
use tera::{Context, Tera};
use tower_http::services::ServeDir;

// --- Tera einmalig halten ---
static TERA: OnceCell<Arc<Tera>> = OnceCell::new();

fn first_existing(paths: &[&str]) -> Option<PathBuf> {
    for p in paths {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Some(pb);
        }
    }
    None
}

fn resolve_templates_root() -> PathBuf {
    // Bevorzugt dein Tailwind-Frontend unter src/web/frontend/templates
    first_existing(&[
        "src/web/frontend/templates",
        "web/frontend/templates",
        "/usr/share/openresquesim/web/frontend/templates",
    ])
    .expect("no templates folder found (tried src/web/frontend/templates, web/frontend/templates, /usr/share/openresquesim/web/frontend/templates)")
}

fn resolve_static_root_from_templates(tpl_root: &Path) -> PathBuf {
    // ../static neben templates
    // src/web/frontend/templates -> src/web/frontend/static
    tpl_root
        .parent()
        .map(|p| p.join("static"))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("src/web/frontend/static"))
}

fn ensure_tera_loaded(tpl_root: &Path) -> Arc<Tera> {
    TERA.get_or_init(|| {
        let glob = tpl_root.join("**/*").to_string_lossy().to_string();
        let tera = Tera::new(&glob).expect("failed to init Tera");
        let arc = Arc::new(tera);
        eprintln!("[web] templates at: {}", tpl_root.display());
        arc
    })
    .clone()
}

// --- Hilfs-Renderer ---
fn render(tera: &Tera, tpl: &str, ctx: &Context) -> Html<String> {
    match tera.render(tpl, ctx) {
        Ok(s) => Html(s),
        Err(e) => {
            eprintln!("render {tpl}: {e:?}");
            Html(format!("<h1>Template error</h1><pre>{e:?}</pre>"))
        }
    }
}

// --- Seiten-Handler (Tera) ---
async fn ui_index(Extension(tera): Extension<Arc<Tera>>) -> Html<String> {
    render(&tera, "index.html", &Context::new())
}
async fn ui_viewer(Extension(tera): Extension<Arc<Tera>>) -> Html<String> {
    render(&tera, "viewer.html", &Context::new())
}
async fn ui_viewer_qr(Extension(tera): Extension<Arc<Tera>>) -> Html<String> {
    render(&tera, "viewer_qr.html", &Context::new())
}
async fn ui_options(Extension(tera): Extension<Arc<Tera>>) -> Html<String> {
    render(&tera, "options.html", &Context::new())
}
async fn ui_user_stats(Extension(tera): Extension<Arc<Tera>>) -> Html<String> {
    render(&tera, "user_stats.html", &Context::new())
}
async fn ui_achievements(Extension(tera): Extension<Arc<Tera>>) -> Html<String> {
    render(&tera, "achievements.html", &Context::new())
}
async fn ui_system(Extension(tera): Extension<Arc<Tera>>) -> Html<String> {
    render(&tera, "system.html", &Context::new())
}
async fn ui_simulate(Extension(tera): Extension<Arc<Tera>>) -> Html<String> {
    render(&tera, "simulate.html", &Context::new())
}
async fn ui_debrief(Extension(tera): Extension<Arc<Tera>>) -> Html<String> {
    render(&tera, "debrief.html", &Context::new())
}
async fn ui_setup(Extension(tera): Extension<Arc<Tera>>) -> Html<String> {
    render(&tera, "setup.html", &Context::new())
}
async fn ui_participants(Extension(tera): Extension<Arc<Tera>>) -> Html<String> {
    render(&tera, "participants.html", &Context::new())
}
async fn ui_camera_control(Extension(tera): Extension<Arc<Tera>>) -> Html<String> {
    render(&tera, "camera_control.html", &Context::new())
}

/// Teilnehmer-Notizblatt (öffentlich per Token-Link). `mode` = edit | print.
fn tn_sheet_ctx(token: &str, mode: &str) -> Context {
    let mut ctx = Context::new();
    // Token nur in gültiger Form durchreichen (Hex), sonst leer → Seite zeigt Fehler
    let clean: String = token.chars().filter(|c| c.is_ascii_hexdigit()).take(64).collect();
    ctx.insert("token", &clean);
    ctx.insert("mode", mode);
    ctx
}
async fn tn_sheet_edit(
    Extension(tera): Extension<Arc<Tera>>,
    axum::extract::Path(token): axum::extract::Path<String>,
) -> Html<String> {
    render(&tera, "tn_sheet.html", &tn_sheet_ctx(&token, "edit"))
}
async fn tn_sheet_print(
    Extension(tera): Extension<Arc<Tera>>,
    axum::extract::Path(token): axum::extract::Path<String>,
) -> Html<String> {
    render(&tera, "tn_sheet.html", &tn_sheet_ctx(&token, "print"))
}

/// Mountet die Web-UI (Templates + Static + HLS) unter /ui.
/// Generisch in `S`, damit dein vorhandener `Router<S>` inkl. State unverändert bleibt.
pub fn mount_web<S>(api: Router<S>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    let tpl_root = resolve_templates_root();
    let static_root = resolve_static_root_from_templates(&tpl_root);
    let static_dir = resolve_static_root_from_templates(&tpl_root);
    // Hilfe (statisches HTML) liegt neben templates/static: src/web/frontend/help
    let help_dir = tpl_root
        .parent()
        .map(|p| p.join("help"))
        .unwrap_or_else(|| PathBuf::from("src/web/frontend/help"));
    let tera = ensure_tera_loaded(&tpl_root);

    api.route("/ui", get(ui_index))
        .route("/ui/viewer", get(ui_viewer))
        .route("/ui/viewer_qr", get(ui_viewer_qr))
        .route("/ui/options", get(ui_options))
        .route("/ui/options/stats", get(ui_user_stats))
        .route("/ui/options/achievements", get(ui_achievements))
        .route("/ui/options/system", get(ui_system))
        .route("/ui/simulate", get(ui_simulate))
        .route("/ui/debrief", get(ui_debrief))
        .route("/setup", get(ui_setup))
        .route("/ui/participants", get(ui_participants))
        .route("/ui/camera-control", get(ui_camera_control))
        .route("/tn/:token", get(tn_sheet_edit))
        .route("/tn/:token/download", get(tn_sheet_print))
        // Statische Assets (CSS, JS, Bilder)
        .nest_service("/static", ServeDir::new(&static_dir)) // optional fallback
        .nest_service("/ui/static", ServeDir::new(static_root))
        .nest_service("/help", ServeDir::new(help_dir))
        // (kein zusätzliches Streaming-Verzeichnis nötig)
        // Tera bereitstellen (Extension ändert den State-Typ NICHT)
        .layer(Extension(tera))
}
