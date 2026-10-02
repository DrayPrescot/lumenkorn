//! Eigenes URL-Schema `lw://` für Vorschaubilder.
//!
//! Das Frontend lädt Bilder als `<img src="lw://localhost/thumb/42">`
//! (unter Windows/Android `http://lw.localhost/thumb/42`). So fließen keine
//! Bilddaten durch die JSON-IPC, und der Browser-Cache greift.
//! Fehlt eine Vorschau, wird sie bei Bedarf erzeugt.

use crate::AppState;
use lumenkorn_core::thumbs::Variant;
use std::sync::{Arc, Condvar, Mutex};
use tauri::http::{Request, Response, StatusCode, header};
use tauri::{Manager, UriSchemeContext, UriSchemeResponder};

pub fn handle<R: tauri::Runtime>(
    ctx: UriSchemeContext<'_, R>,
    req: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let app = ctx.app_handle().clone();
    let path = req.uri().path().to_string();

    // Dekodieren kann dauern – nie im Event-Loop. Feste Arbeiter statt eines
    // Threads je Anfrage (Scrollen durch 50 000 Fotos erzeugte Tausende), die
    // neueste Anfrage zuerst: Das sind die Bilder, die gerade sichtbar sind.
    // Volle Auflösung (bis ~1 GB je Bild) in einem eigenen Pool mit zwei Plätzen.
    let heavy = path.trim_start_matches('/').starts_with("full/");
    let pool = if heavy { &FULL_POOL } else { &POOL };
    pool.submit(Box::new(move || {
        let state = app.state::<AppState>();
        responder.respond(serve(&state, &path));
    }));
}

type Task = Box<dyn FnOnce() + Send>;

/// Kleiner Thread-Pool mit LIFO-Warteschlange.
struct Pool {
    threads: fn() -> usize,
    queue: Mutex<Vec<Task>>,
    ready: Condvar,
    started: std::sync::Once,
}

impl Pool {
    const fn new(threads: fn() -> usize) -> Self {
        Self {
            threads,
            queue: Mutex::new(Vec::new()),
            ready: Condvar::new(),
            started: std::sync::Once::new(),
        }
    }

    fn submit(&'static self, task: Task) {
        self.started.call_once(|| {
            for i in 0..(self.threads)() {
                let _ = std::thread::Builder::new()
                    .name(format!("lumenkorn-lw-{i}"))
                    .spawn(move || self.work());
            }
        });
        self.queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(task);
        self.ready.notify_one();
    }

    fn work(&'static self) {
        loop {
            let task = {
                let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
                loop {
                    if let Some(t) = q.pop() {
                        break t;
                    }
                    q = self.ready.wait(q).unwrap_or_else(|e| e.into_inner());
                }
            };
            // Eine Panik (kaputte Datei) darf den Arbeiter nicht beenden
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(task)).is_err() {
                log::error!("Bildanfrage abgebrochen (Panik)");
            }
        }
    }
}

static POOL: Pool = Pool::new(|| {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(2, 8)
});
static FULL_POOL: Pool = Pool::new(|| 2);

/// Zuletzt gelieferte Bilder in voller Auflösung: (ID, Bearbeitung, JPEG).
/// Beim Vergleichen wird zwischen wenigen Bildern hin- und hergezoomt.
/// Schlüssel enthält Größe und Änderungszeit der Datei: Wird sie außerhalb
/// geändert, gilt das zwischengespeicherte Bild nicht mehr.
type FullKey = (i64, Option<(u64, std::time::SystemTime)>);
type FullEntry = (FullKey, Option<serde_json::Value>, Arc<Vec<u8>>);
static FULL: Mutex<Vec<FullEntry>> = Mutex::new(Vec::new());
const FULL_ENTRIES: usize = 6;

fn serve_full(state: &AppState, id: i64) -> Response<Vec<u8>> {
    let job = match state.catalog.lock().unwrap().thumb_job(id) {
        Ok(j) => j,
        Err(_) => return status(StatusCode::NOT_FOUND),
    };
    let stamp = std::fs::metadata(&job.path)
        .ok()
        .and_then(|m| Some((m.len(), m.modified().ok()?)));
    let key = (id, stamp);
    {
        let c = FULL.lock().unwrap();
        if let Some((_, _, bytes)) = c.iter().find(|(k, e, _)| *k == key && *e == job.edit) {
            return jpeg(bytes.to_vec());
        }
    }
    let t = std::time::Instant::now();
    match state.cache.render_full(&job) {
        Ok(bytes) => {
            log::info!("Volle Auflösung für Bild {id} in {:.2?}", t.elapsed());
            let mut c = FULL.lock().unwrap();
            c.retain(|(k, _, _)| k.0 != id);
            c.insert(0, (key, job.edit.clone(), Arc::new(bytes.clone())));
            c.truncate(FULL_ENTRIES);
            jpeg(bytes)
        }
        Err(e) => {
            log::warn!("Volle Auflösung {id}: {e}");
            status(StatusCode::UNPROCESSABLE_ENTITY)
        }
    }
}

fn serve(state: &AppState, path: &str) -> Response<Vec<u8>> {
    let mut parts = path.trim_start_matches('/').split('/');
    let first = parts.next();
    if first == Some("face") {
        return match parts
            .next()
            .and_then(|s| s.parse::<i64>().ok())
            .and_then(|id| crate::people::face_thumb(state, id))
        {
            Some(b) => jpeg(b),
            None => status(StatusCode::NOT_FOUND),
        };
    }
    if first == Some("full") {
        return match parts.next().and_then(|s| s.parse::<i64>().ok()) {
            Some(id) => serve_full(state, id),
            None => status(StatusCode::BAD_REQUEST),
        };
    }
    let variant = match first {
        Some("thumb") => Variant::Thumb,
        Some("preview") => Variant::Preview,
        _ => return status(StatusCode::NOT_FOUND),
    };
    let Some(id) = parts.next().and_then(|s| s.parse::<i64>().ok()) else {
        return status(StatusCode::BAD_REQUEST);
    };

    // Schnellweg: liegt im Cache
    if let Some(bytes) = state.cache.read(variant, id) {
        return jpeg(bytes);
    }

    let job = match state.catalog.lock().unwrap().thumb_job(id) {
        Ok(j) => j,
        Err(_) => return status(StatusCode::NOT_FOUND),
    };
    match state.cache.get_or_render(variant, &job) {
        Ok(bytes) => {
            if variant == Variant::Thumb {
                let _ = state.catalog.lock().unwrap().set_thumb_state(id, true);
            }
            jpeg(bytes)
        }
        Err(e) => {
            log::warn!("Vorschau {id}: {e}");
            status(StatusCode::UNPROCESSABLE_ENTITY)
        }
    }
}

fn jpeg(bytes: Vec<u8>) -> Response<Vec<u8>> {
    Response::builder()
        .header(header::CONTENT_TYPE, "image/jpeg")
        // Die URL enthält eine Version (?v=…), ändert sich also bei neuem Inhalt.
        .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(bytes)
        .unwrap()
}

fn status(code: StatusCode) -> Response<Vec<u8>> {
    Response::builder().status(code).body(Vec::new()).unwrap()
}
