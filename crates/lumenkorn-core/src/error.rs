use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Datenbankfehler: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("Ein-/Ausgabefehler: {0}")]
    Io(#[from] std::io::Error),
    #[error("Bildfehler: {0}")]
    Image(#[from] image::ImageError),
    #[error("Dateiüberwachung: {0}")]
    Watch(#[from] notify::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Develop(#[from] lumenkorn_develop::Error),
    #[error("Nicht gefunden: {0}")]
    NotFound(String),
    #[error("Ungültige Eingabe: {0}")]
    Invalid(String),
    #[error("Keine Vorschau im RAW gefunden: {0}")]
    NoPreview(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Fehler werden an die Oberfläche als Text weitergereicht.
impl serde::Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}
