use lumenkorn_develop::i18n::tr_s;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("{p}: {0}", p = tr_s("Datenbankfehler", "Database error"))]
    Db(#[from] rusqlite::Error),
    #[error("{p}: {0}", p = tr_s("Ein-/Ausgabefehler", "I/O error"))]
    Io(#[from] std::io::Error),
    #[error("{p}: {0}", p = tr_s("Bildfehler", "Image error"))]
    Image(#[from] image::ImageError),
    #[error("{p}: {0}", p = tr_s("Dateiüberwachung", "File watching"))]
    Watch(#[from] notify::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Develop(#[from] lumenkorn_develop::Error),
    #[error("{p}: {0}", p = tr_s("Nicht gefunden", "Not found"))]
    NotFound(String),
    #[error("{p}: {0}", p = tr_s("Ungültige Eingabe", "Invalid input"))]
    Invalid(String),
    #[error("{p}: {0}", p = tr_s("Keine Vorschau im RAW gefunden", "No preview found in RAW"))]
    NoPreview(String),
    /// Katalog von einer neueren Lumenkorn-Version (Schema `version`, diese
    /// Version kennt bis `known`) – die App bietet dann das Update an.
    #[error("{}", if lumenkorn_develop::i18n::is_english() {
        format!("The catalog was created by a newer Lumenkorn version (schema {version}, this version knows up to {known}). Please update Lumenkorn.")
    } else {
        format!("Der Katalog stammt von einer neueren Lumenkorn-Version (Stand {version}, diese Version kennt bis {known}). Bitte Lumenkorn aktualisieren.")
    })]
    CatalogTooNew { version: i64, known: usize },
}

pub type Result<T> = std::result::Result<T, Error>;

/// Fehler werden an die Oberfläche als Text weitergereicht.
impl serde::Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}
