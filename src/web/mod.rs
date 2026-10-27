pub mod pager;
pub mod viewer;

use std::borrow::Cow;

/// Textdatei einer fest eingebauten Seite (Viewer, Pager) liefern.
///
/// Im Entwicklungsbaum (Server läuft im Projektordner) wird `src/web/<page>/<file>` frisch
/// von der Platte gelesen, Änderungen wirken dann wie beim übrigen Frontend sofort. Im
/// Paket gibt es den Ordner nicht, dort gilt die beim Bauen eingebettete Fassung – das
/// Debian-Paket bleibt so ohne zusätzliche Dateien lauffähig.
pub fn page_text(page: &str, file: &str, embedded: &'static str) -> Cow<'static, str> {
    let path = std::path::Path::new("src/web").join(page).join(file);
    match std::fs::read_to_string(&path) {
        Ok(s) => Cow::Owned(s),
        Err(_) => Cow::Borrowed(embedded),
    }
}
