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
        }),
        model: vec![
            FileSpec {
                name: "vision.onnx".into(),
                url: format!("{base}/v.onnx"),
                size: None,
            },
            FileSpec {
                name: "text.onnx".into(),
                url: format!("{base}/t.onnx"),
                size: None,
            },
            FileSpec {
                name: "vocab.json".into(),
                url: format!("{base}/vocab.json"),
                size: None,
            },
            FileSpec {
                name: "merges.txt".into(),
                url: format!("{base}/merges.txt"),
                size: None,
            },
        ],
        masks: vec![FileSpec {
            name: "subject.onnx".into(),
            url: format!("{base}/t.onnx"),
            size: None,
        }],
        denoise: vec![],
        gpu_runtimes: vec![],
        faces: vec![],
        repair: vec![],
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
        }],
        masks: vec![],
        denoise: vec![],
        gpu_runtimes: vec![],
        faces: vec![],
        repair: vec![],
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
