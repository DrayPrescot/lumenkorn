use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Ein-/Ausgabefehler: {0}")]
    Io(#[from] std::io::Error),
    #[error("Bildfehler: {0}")]
    Image(#[from] image::ImageError),
    #[error("RAW-Datei konnte nicht entwickelt werden: {0}")]
    Raw(String),
    #[error("Ungültige Parameter: {0}")]
    Params(String),
    #[error("JPEG-Kodierung: {0}")]
    Jpeg(#[from] jpeg_encoder::EncodingError),
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<rawler::RawlerError> for Error {
    fn from(e: rawler::RawlerError) -> Self {
        Error::Raw(e.to_string())
    }
}
