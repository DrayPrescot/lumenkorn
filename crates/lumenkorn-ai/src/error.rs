use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("ONNX Runtime: {0}")]
    Ort(String),
    #[error("ONNX Runtime ist nicht geladen")]
    RuntimeMissing,
    #[error("Modell unvollständig: {0} fehlt")]
    ModelMissing(String),
    #[error("Unerwartetes Modellformat: {0}")]
    ModelFormat(String),
    #[error("Tokenizer: {0}")]
    Tokenizer(String),
    #[error("Ein-/Ausgabefehler: {0}")]
    Io(#[from] std::io::Error),
    #[error("Bildfehler: {0}")]
    Image(#[from] image::ImageError),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Download fehlgeschlagen: {0}")]
    Download(String),
    #[error("Abgebrochen")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, Error>;

impl<T> From<ort::Error<T>> for Error {
    fn from(e: ort::Error<T>) -> Self {
        Error::Ort(e.to_string())
    }
}
