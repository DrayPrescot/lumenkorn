use crate::i18n::tr_s;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("{p}: {0}", p = tr_s("Ein-/Ausgabefehler", "I/O error"))]
    Io(#[from] std::io::Error),
    #[error("{p}: {0}", p = tr_s("Bildfehler", "Image error"))]
    Image(#[from] image::ImageError),
    #[error("{p}: {0}", p = tr_s("RAW-Datei konnte nicht entwickelt werden", "Could not develop RAW file"))]
    Raw(String),
    #[error("{p}: {0}", p = tr_s("Ungültige Parameter", "Invalid parameters"))]
    Params(String),
    #[error("{p}: {0}", p = tr_s("JPEG-Kodierung", "JPEG encoding"))]
    Jpeg(#[from] jpeg_encoder::EncodingError),
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<rawler::RawlerError> for Error {
    fn from(e: rawler::RawlerError) -> Self {
        Error::Raw(e.to_string())
    }
}
