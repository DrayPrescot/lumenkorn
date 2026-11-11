//! Diashow: Hintergrundmusik laden und das fertige Video schreiben.
//!
//! Die Oberfläche dekodiert die Musik selbst (Web Audio), erkennt darin den
//! Takt und spielt sie ab; hierher kommt nur das Lesen der gewählten Datei.
//! Das Video kodiert die WebView (WebCodecs: H.264/AAC über die Encoder von
//! Windows bzw. der Grafikkarte) und schickt die MP4-Daten stückweise mit
//! ihrer Position in der Datei – der Muxer trägt den Dateikopf am Ende nach.

use crate::blocking;
use lumenkorn_core::{Error, LockExt};
use lumenkorn_develop::i18n::tr_s;
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::AppHandle;
use tauri::ipc::{InvokeBody, Request, Response};

/// Das Video, das gerade geschrieben wird (es gibt immer nur eins)
static VIDEO: Mutex<Option<(PathBuf, File)>> = Mutex::new(None);

/// Endungen, die die WebView dekodieren kann
pub const MUSIC_EXTENSIONS: &[&str] = &["mp3", "m4a", "aac", "wav", "flac", "ogg", "opus"];
/// Größte Musikdatei (Bytes) – längere Stücke als Album-Länge braucht keine Diashow
const MAX_MUSIC_BYTES: u64 = 300 * 1024 * 1024;

fn is_music(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| MUSIC_EXTENSIONS.iter().any(|m| m.eq_ignore_ascii_case(e)))
}

/// Inhalt einer Musikdatei (nur Audio-Endungen, begrenzte Größe).
#[tauri::command]
pub async fn slideshow_music(app: AppHandle, path: String) -> Result<Response, Error> {
    blocking(app, move |_| {
        let p = Path::new(&path);
        if !is_music(p) {
            return Err(Error::Invalid(
                tr_s(
                    "Keine unterstützte Musikdatei",
                    "Not a supported music file",
                )
                .into(),
            ));
        }
        let meta = std::fs::metadata(p).map_err(|_| Error::NotFound(path.clone()))?;
        if meta.len() > MAX_MUSIC_BYTES {
            return Err(Error::Invalid(
                tr_s("Die Musikdatei ist zu groß", "The music file is too large").into(),
            ));
        }
        Ok(Response::new(std::fs::read(p)?))
    })
    .await
}

fn no_video() -> Error {
    Error::Invalid(
        tr_s(
            "Es wird gerade kein Video gespeichert",
            "No video is being saved",
        )
        .into(),
    )
}

/// Videodatei zum Schreiben anlegen (nur `.mp4`).
#[tauri::command]
pub async fn slideshow_video_open(path: String) -> Result<(), Error> {
    let p = PathBuf::from(&path);
    let mp4 = p
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("mp4"));
    if !mp4 {
        return Err(Error::Invalid(
            tr_s(
                "Das Video muss die Endung .mp4 haben",
                "The video must end in .mp4",
            )
            .into(),
        ));
    }
    // Erst unter einem Zwischennamen schreiben: Ein Abbruch (oder Absturz) ließe
    // sonst ein unbrauchbares Video unter dem Zielnamen zurück und hätte ein
    // vorhandenes schon überschrieben
    let part = part_path(&p);
    let file = File::create(&part)?;
    // Ein abgebrochenes früheres Video bleibt nicht halb geschrieben liegen
    if let Some((old, f)) = VIDEO.locked().replace((p, file)) {
        drop(f);
        let old = part_path(&old);
        if old != part {
            let _ = std::fs::remove_file(old);
        }
    }
    Ok(())
}

/// Zwischenname des Videos `p` während des Schreibens
fn part_path(p: &std::path::Path) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(".part");
    PathBuf::from(s)
}

/// Ein Stück der MP4-Daten an seine Position schreiben. Körper: die Bytes,
/// Kopfzeile `x-position`: Position in der Datei.
#[tauri::command]
pub async fn slideshow_video_write(request: Request<'_>) -> Result<(), Error> {
    let InvokeBody::Raw(data) = request.body() else {
        return Err(Error::Invalid(
            "slideshow_video_write: raw body expected".into(),
        ));
    };
    let pos: u64 = request
        .headers()
        .get("x-position")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| Error::Invalid("slideshow_video_write: x-position".into()))?;
    let mut video = VIDEO.locked();
    let (_, file) = video.as_mut().ok_or_else(no_video)?;
    file.seek(SeekFrom::Start(pos))?;
    file.write_all(data)?;
    Ok(())
}

/// Video abschließen; `keep = false` (abgebrochen, Fehler): Datei löschen.
#[tauri::command]
pub async fn slideshow_video_close(keep: bool) -> Result<(), Error> {
    let Some((path, file)) = VIDEO.locked().take() else {
        return Ok(());
    };
    let part = part_path(&path);
    if keep {
        let done = file.sync_all();
        drop(file);
        // Erst jetzt ersetzt das fertige Video ein vorhandenes gleichen Namens
        if let Err(e) = done.and_then(|()| std::fs::rename(&part, &path)) {
            let _ = std::fs::remove_file(&part);
            return Err(e.into());
        }
    } else {
        drop(file);
        let _ = std::fs::remove_file(part);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_audio_files() {
        assert!(is_music(Path::new("C:/Musik/Lied.MP3")));
        assert!(is_music(Path::new("/home/a/x.flac")));
        assert!(!is_music(Path::new("/etc/passwd")));
        assert!(!is_music(Path::new("C:/Fotos/bild.jpg")));
    }
}
