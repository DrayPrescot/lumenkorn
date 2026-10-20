use lumenkorn_develop::i18n::tr_s;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("ONNX Runtime: {0}")]
    Ort(String),
    #[error("{p}", p = tr_s("ONNX Runtime ist nicht geladen", "ONNX Runtime is not loaded"))]
    RuntimeMissing,
    #[error("{p}: {0} {q}", p = tr_s("Modell unvollständig", "Model incomplete"), q = tr_s("fehlt", "missing"))]
    ModelMissing(String),
    #[error("{p}: {0}", p = tr_s("Unerwartetes Modellformat", "Unexpected model format"))]
    ModelFormat(String),
    #[error("Tokenizer: {0}")]
    Tokenizer(String),
    #[error("{p}: {0}", p = tr_s("Ein-/Ausgabefehler", "I/O error"))]
    Io(#[from] std::io::Error),
    #[error("{p}: {0}", p = tr_s("Bildfehler", "Image error"))]
    Image(#[from] image::ImageError),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{p}: {0}", p = tr_s("Download fehlgeschlagen", "Download failed"))]
    Download(String),
    #[error("{p}: {0}", p = tr_s("Nicht genug Arbeitsspeicher", "Not enough memory"))]
    OutOfMemory(String),
    // Die Oberfläche erkennt Abbrüche an „Abgebrochen“/„Canceled“ (isCanceled)
    #[error("{p}", p = tr_s("Abgebrochen", "Canceled"))]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, Error>;

impl<T> From<ort::Error<T>> for Error {
    fn from(e: ort::Error<T>) -> Self {
        Error::Ort(e.to_string())
    }
}
