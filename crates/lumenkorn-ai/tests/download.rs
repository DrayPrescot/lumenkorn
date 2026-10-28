//! Download-Logik gegen einen lokalen HTTP-Server – ohne Internet.

use lumenkorn_ai::models::{self, Component, FileSpec, Manifest, RuntimeSpec};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::AtomicBool;

/// Minimaler HTTP-Server: liefert pro Pfad den hinterlegten Inhalt.
fn serve(files: Vec<(&'static str, Vec<u8>)>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut buf = [0u8; 4096];
            let n = s.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
            match files.iter().find(|(p, _)| *p == path) {
                Some((_, body)) => {
                    let _ = write!(
                        s,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = s.write_all(body);
                }
                None => {
                    let _ = s.write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                }
            }
        }
    });
    format!("http://{addr}")
}

fn tgz_with(entry: &str, content: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let gz = flate2::write::GzEncoder::new(&mut out, flate2::Compression::fast());
        let mut tar = tar::Builder::new(gz);
        let mut h = tar::Header::new_gnu();
        h.set_size(content.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        tar.append_data(&mut h, entry, content).unwrap();
        tar.into_inner().unwrap().finish().unwrap();
    }
    out
}

#[test]
fn downloads_runtime_and_model_with_progress() {
    let big: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    let lib = b"fake library".to_vec();
    let base = serve(vec![
        (
            "/ort.tgz",
            tgz_with("onnxruntime-x/lib/libonnxruntime.so.9.9", &lib),
        ),
        ("/v.onnx", big.clone()),
        ("/t.onnx", b"text".to_vec()),
        ("/vocab.json", b"{}".to_vec()),
        ("/merges.txt", b"#version: 0.2\n".to_vec()),
    ]);

    let data = tempfile::tempdir().unwrap();
    let ai = models::ai_dir(data.path());
    std::fs::create_dir_all(&ai).unwrap();
    let manifest = Manifest {
        runtime: Some(RuntimeSpec {
            url: format!("{base}/ort.tgz"),
            archive_entry: "lib/libonnxruntime.so.9.9".into(),
            size: None,
            sha256: None,
        }),
        model: vec![
            FileSpec {
                name: "vision.onnx".into(),
                url: format!("{base}/v.onnx"),
                size: None,
                sha256: None,
            },
            FileSpec {
                name: "text.onnx".into(),
                url: format!("{base}/t.onnx"),
                size: None,
                sha256: None,
            },
            FileSpec {
                name: "vocab.json".into(),
                url: format!("{base}/vocab.json"),
                size: None,
                sha256: None,
            },
            FileSpec {
                name: "merges.txt".into(),
                url: format!("{base}/merges.txt"),
                size: None,
                sha256: None,
            },
        ],
        masks: vec![FileSpec {
            name: "subject.onnx".into(),
            url: format!("{base}/t.onnx"),
            size: None,
            sha256: None,
        }],
        denoise: vec![],
        gpu_runtimes: vec![],
        faces: vec![],
        repair: vec![],
        animals: vec![],
    };
    std::fs::write(
        ai.join("quellen.json"),
        serde_json::to_string(&manifest).unwrap(),
    )
    .unwrap();

    let before = models::status(data.path());
    assert!(!before.model_ready);
    assert_eq!(before.missing.len(), 4);

    let seen = std::sync::Mutex::new(Vec::new());
    models::download_missing(
        data.path(),
        Component::Clip,
        &|p| seen.lock().unwrap().push((p.item, p.done, p.total)),
        &AtomicBool::new(false),
    )
    .unwrap();

    let after = models::status(data.path());
    assert!(after.model_ready, "{:?}", after.missing);
    let rt = ai
        .join("runtime")
        .join(lumenkorn_ai::runtime::library_file_name());
    if cfg!(target_os = "linux") {
        assert_eq!(std::fs::read(&rt).unwrap(), lib);
    }
    assert_eq!(
        std::fs::read(models::model_dir(data.path()).join("vision.onnx")).unwrap(),
        big
    );
    let seen = seen.lock().unwrap();
    assert!(seen.iter().any(|(item, done, total)| item == "vision.onnx"
        && *done == 3_000_000
        && *total == Some(3_000_000)));
    // Keine Reste
    assert!(
        std::fs::read_dir(models::model_dir(data.path()))
            .unwrap()
            .all(|e| !e.unwrap().file_name().to_string_lossy().ends_with(".part"))
    );
}

#[test]
fn cancel_and_404_leave_no_partial_files() {
    let base = serve(vec![]);
    let data = tempfile::tempdir().unwrap();
    let ai = models::ai_dir(data.path());
    std::fs::create_dir_all(&ai).unwrap();
    let manifest = Manifest {
        runtime: None,
        model: vec![FileSpec {
            name: "vision.onnx".into(),
            url: format!("{base}/fehlt.onnx"),
            size: None,
            sha256: None,
        }],
        masks: vec![],
        denoise: vec![],
        gpu_runtimes: vec![],
        faces: vec![],
        repair: vec![],
        animals: vec![],
    };
    std::fs::write(
        ai.join("quellen.json"),
        serde_json::to_string(&manifest).unwrap(),
    )
    .unwrap();
    assert!(
        models::download_missing(
            data.path(),
            Component::Clip,
            &|_| {},
            &AtomicBool::new(false)
        )
        .is_err()
    );
    assert!(!models::model_dir(data.path()).join("vision.onnx").exists());
}

/// Viel kleiner als angegeben (abgeschnitten, Server ohne Längenangabe):
/// nicht als fertiges Modell übernehmen.
#[test]
fn truncated_download_is_rejected() {
    let base = serve(vec![("/v.onnx", vec![7u8; 1000])]);
    let data = tempfile::tempdir().unwrap();
    let ai = models::ai_dir(data.path());
    std::fs::create_dir_all(&ai).unwrap();
    let manifest = Manifest {
        runtime: None,
        model: vec![FileSpec {
            name: "vision.onnx".into(),
            url: format!("{base}/v.onnx"),
            size: Some(50_000),
            sha256: None,
        }],
        masks: vec![],
        denoise: vec![],
        gpu_runtimes: vec![],
        faces: vec![],
        repair: vec![],
        animals: vec![],
    };
    std::fs::write(
        ai.join("quellen.json"),
        serde_json::to_string(&manifest).unwrap(),
    )
    .unwrap();
    let res = models::download_missing(
        data.path(),
        Component::Clip,
        &|_| {},
        &AtomicBool::new(false),
    );
    assert!(res.is_err(), "abgeschnitten angenommen");
    let dir = models::model_dir(data.path());
    assert!(!dir.join("vision.onnx").exists());
    assert!(!dir.join("vision.onnx.part").exists());
}

#[test]
fn gpu_runtime_from_wheel_checks_sha256() {
    use lumenkorn_ai::accel::{self, Backend};
    use lumenkorn_ai::models::GpuRuntimeSpec;
    use sha2::Digest;

    // Gefälschtes Wheel: Bibliothek mit Versionsnummer + Begleiter
    let mut wheel = Vec::new();
    {
        let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut wheel));
        let opt = zip::write::SimpleFileOptions::default();
        for (name, body) in [
            (
                "onnxruntime/capi/libonnxruntime.so.1.27.0",
                b"gpu lib".as_slice(),
            ),
            (
                "onnxruntime/capi/libonnxruntime_providers_shared.so",
                b"shared",
            ),
            ("onnxruntime/capi/onnxruntime_pybind11_state.so", b"python"),
        ] {
            z.start_file(name, opt).unwrap();
            z.write_all(body).unwrap();
        }
        z.finish().unwrap();
    }
    let sha: String = sha2::Sha256::digest(&wheel)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let base = serve(vec![("/ort.whl", wheel)]);

    let install = |sha: &str| {
        let data = tempfile::tempdir().unwrap();
        let ai = models::ai_dir(data.path());
        std::fs::create_dir_all(&ai).unwrap();
        let manifest = Manifest {
            runtime: None,
            model: vec![],
            masks: vec![],
            denoise: vec![],
            gpu_runtimes: vec![GpuRuntimeSpec {
                backend: Backend::Webgpu,
                version: "1.27.0".into(),
                url: format!("{base}/ort.whl"),
                sha256: Some(sha.into()),
                size: None,
                files: vec![
                    "onnxruntime/capi/libonnxruntime.so.1.27.0".into(),
                    "onnxruntime/capi/libonnxruntime_providers_shared.so".into(),
                ],
            }],
            faces: vec![],
            repair: vec![],
            animals: vec![],
        };
        std::fs::write(
            ai.join("quellen.json"),
            serde_json::to_string(&manifest).unwrap(),
        )
        .unwrap();
        let res = models::download_missing(
            data.path(),
            Component::Webgpu,
            &|_| {},
            &AtomicBool::new(false),
        );
        (data, res)
    };

    let (data, res) = install(&sha);
    res.unwrap();
    let ai = models::ai_dir(data.path());
    let dir = accel::runtime_dir(&ai, Backend::Webgpu);
    if cfg!(target_os = "linux") {
        assert_eq!(
            std::fs::read(dir.join("libonnxruntime.so")).unwrap(),
            b"gpu lib"
        );
        assert!(dir.join("libonnxruntime_providers_shared.so").is_file());
        let st = models::status(data.path());
        assert!(st.gpu_runtimes.iter().any(|g| g.installed));
    }
    assert!(!dir.join("onnxruntime_pybind11_state.so").exists());

    // Falsche Prüfsumme: nichts installiert, keine Reste
    let (data, res) = install(&"0".repeat(64));
    assert!(res.unwrap_err().to_string().contains("Prüfsumme"));
    let ai = models::ai_dir(data.path());
    assert!(!accel::runtime_dir(&ai, Backend::Webgpu).exists());
    assert!(
        std::fs::read_dir(&ai).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("part"))
    );
}

fn single_file_manifest(data: &std::path::Path, spec: FileSpec) {
    let ai = models::ai_dir(data);
    std::fs::create_dir_all(&ai).unwrap();
    let manifest = Manifest {
        runtime: None,
        model: vec![spec],
        masks: vec![],
        denoise: vec![],
        gpu_runtimes: vec![],
        faces: vec![],
        repair: vec![],
        animals: vec![],
    };
    std::fs::write(
        ai.join("quellen.json"),
        serde_json::to_string(&manifest).unwrap(),
    )
    .unwrap();
}

#[test]
fn model_checksum_is_verified_on_fresh_download() {
    use sha2::Digest;
    let body = vec![3u8; 5000];
    let sha: String = sha2::Sha256::digest(&body)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let base = serve(vec![("/v.onnx", body.clone())]);
    let run = |sha: &str| {
        let data = tempfile::tempdir().unwrap();
        single_file_manifest(
            data.path(),
            FileSpec {
                name: "vision.onnx".into(),
                url: format!("{base}/v.onnx"),
                size: Some(5000),
                sha256: Some(sha.into()),
            },
        );
        let res = models::download_missing(
            data.path(),
            Component::Clip,
            &|_| {},
            &AtomicBool::new(false),
        );
        (data, res)
    };
    let (data, res) = run(&sha);
    res.unwrap();
    let dir = models::model_dir(data.path());
    assert_eq!(std::fs::read(dir.join("vision.onnx")).unwrap(), body);
    assert!(!dir.join("vision.onnx.part.src").exists());

    // Falsche Prüfsumme: Datei wird nicht übernommen, Teil samt Marker gelöscht
    let (data, res) = run(&"0".repeat(64));
    assert!(res.unwrap_err().to_string().contains("Prüfsumme"));
    let dir = models::model_dir(data.path());
    assert!(!dir.join("vision.onnx").exists());
    assert!(!dir.join("vision.onnx.part").exists());
    assert!(!dir.join("vision.onnx.part.src").exists());

    // Schon vorhandene Datei: gilt als fertig, wird weder geprüft noch neu geladen
    let data = tempfile::tempdir().unwrap();
    single_file_manifest(
        data.path(),
        FileSpec {
            name: "vision.onnx".into(),
            url: format!("{base}/fehlt"),
            size: Some(5000),
            sha256: Some("0".repeat(64)),
        },
    );
    let dir = models::model_dir(data.path());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("vision.onnx"), b"alt").unwrap();
    models::download_missing(
        data.path(),
        Component::Clip,
        &|_| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(std::fs::read(dir.join("vision.onnx")).unwrap(), b"alt");
}

/// Abbruch greift auch bei hängender Verbindung binnen Sekunden und lässt den
/// Teil für die Fortsetzung liegen.
#[test]
fn cancel_is_prompt_and_keeps_part() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Length: 100000\r\n\r\n");
            let _ = s.write_all(&[1u8; 30_000]);
            let _ = s.flush();
            std::thread::sleep(std::time::Duration::from_secs(20));
        }
    });
    let data = tempfile::tempdir().unwrap();
    single_file_manifest(
        data.path(),
        FileSpec {
            name: "vision.onnx".into(),
            url: format!("http://{addr}/v.onnx"),
            size: Some(100_000),
            sha256: None,
        },
    );
    let cancel = std::sync::Arc::new(AtomicBool::new(false));
    let c2 = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(700));
        c2.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    let started = std::time::Instant::now();
    let res = models::download_missing(data.path(), Component::Clip, &|_| {}, &cancel);
    assert!(
        matches!(res, Err(lumenkorn_ai::Error::Cancelled)),
        "{res:?}"
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    let part = models::model_dir(data.path()).join("vision.onnx.part");
    assert_eq!(std::fs::metadata(part).unwrap().len(), 30_000);
}

/// Ein Teil von einer anderen Quelle wird nicht fortgesetzt.
#[test]
fn part_from_other_source_is_not_resumed() {
    let body: Vec<u8> = (0..9000u32).map(|i| (i % 7) as u8).collect();
    let base = serve(vec![("/v.onnx", body.clone())]);
    let data = tempfile::tempdir().unwrap();
    single_file_manifest(
        data.path(),
        FileSpec {
            name: "vision.onnx".into(),
            url: format!("{base}/v.onnx"),
            size: Some(9000),
            sha256: None,
        },
    );
    let dir = models::model_dir(data.path());
    std::fs::create_dir_all(&dir).unwrap();
    // Teil ohne passenden Marker (z. B. aus einer älteren Version)
    std::fs::write(dir.join("vision.onnx.part"), vec![9u8; 4000]).unwrap();
    models::download_missing(
        data.path(),
        Component::Clip,
        &|_| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(std::fs::read(dir.join("vision.onnx")).unwrap(), body);
}

#[test]
fn manifest_with_unsafe_names_or_urls_is_ignored() {
    let write = |name: &str, url: &str, runtime_sha: bool| {
        let data = tempfile::tempdir().unwrap();
        let ai = models::ai_dir(data.path());
        std::fs::create_dir_all(&ai).unwrap();
        let manifest = Manifest {
            runtime: Some(RuntimeSpec {
                url: url.into(),
                archive_entry: "x".into(),
                size: None,
                sha256: runtime_sha.then(|| "0".repeat(64)),
            }),
            model: vec![FileSpec {
                name: name.into(),
                url: "https://example.org/a".into(),
                size: None,
                sha256: None,
            }],
            masks: vec![],
            denoise: vec![],
            gpu_runtimes: vec![],
            faces: vec![],
            repair: vec![],
            animals: vec![],
        };
        std::fs::write(
            ai.join("quellen.json"),
            serde_json::to_string(&manifest).unwrap(),
        )
        .unwrap();
        models::load_manifest(data.path())
    };
    let custom = |m: &Manifest| m.model.len() == 1 && m.model[0].name != "vision.onnx";
    assert!(custom(&write(
        "eigen.onnx",
        "https://example.org/rt.zip",
        false
    )));
    assert!(custom(&write(
        "eigen.onnx",
        "http://127.0.0.1:9/rt.zip",
        false
    )));
    for bad in ["../x.onnx", "a/b.onnx", r"a\b.onnx", "C:x.onnx", ".."] {
        assert!(
            !custom(&write(bad, "https://example.org/rt.zip", false)),
            "{bad}"
        );
    }
    // Laufzeit ohne Prüfsumme nur über https; mit Prüfsumme genügt http
    assert!(!custom(&write(
        "eigen.onnx",
        "http://example.org/rt.zip",
        false
    )));
    assert!(custom(&write(
        "eigen.onnx",
        "http://example.org/rt.zip",
        true
    )));
    assert!(!custom(&write(
        "eigen.onnx",
        "ftp://example.org/rt.zip",
        true
    )));
}
