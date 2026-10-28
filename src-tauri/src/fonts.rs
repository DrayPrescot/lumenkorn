//! Schriftarten des Betriebssystems für das Fotoalbum. Die Oberfläche zeichnet
//! die Texte selbst (Canvas, auch für das PDF) und kann jede installierte
//! Schrift nutzen – sie braucht nur die Namen der Familien.

use std::sync::OnceLock;

static FAMILIES: OnceLock<Vec<String>> = OnceLock::new();

/// Schriftfamilien des Systems, sortiert und ohne Doppelte. Beim ersten
/// Aufruf werden die Schriftdateien gelesen (nur die Namen), danach kommt
/// die gemerkte Liste.
#[tauri::command(async)]
pub fn system_fonts() -> Vec<String> {
    FAMILIES
        .get_or_init(|| {
            let t = std::time::Instant::now();
            let mut db = fontdb::Database::new();
            db.load_system_fonts();
            let mut names: Vec<String> = db
                .faces()
                // Der erste Name ist der englische, falls vorhanden
                .filter_map(|f| f.families.first().map(|(n, _)| n.trim().to_string()))
                // „@…“: senkrechte Varianten ostasiatischer Schriften (Windows),
                // „.…“: interne Systemschriften (macOS)
                .filter(|n| !n.is_empty() && !n.starts_with('@') && !n.starts_with('.'))
                .collect();
            names.sort_by_key(|n| n.to_lowercase());
            names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
            log::info!(
                "{} Schriftfamilien des Systems gefunden in {:.1?}",
                names.len(),
                t.elapsed()
            );
            names
        })
        .clone()
}

#[cfg(test)]
mod tests {
    #[test]
    fn system_font_names_are_sorted_and_clean() {
        let names = super::system_fonts();
        // Auf Rechnern ohne Schriften (CI) darf die Liste leer sein
        assert!(names.iter().all(|n| !n.is_empty() && !n.starts_with('@')));
        assert!(
            names
                .windows(2)
                .all(|w| w[0].to_lowercase() <= w[1].to_lowercase())
        );
        if cfg!(windows) {
            assert!(names.iter().any(|n| n == "Arial"), "{names:?}");
        }
    }
}
