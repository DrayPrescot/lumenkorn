//! Ende-zu-Ende-Tests des Kerns mit echten Dateien in temporären Ordnern.

use image::{Rgb, RgbImage};
use lumenkorn_core::scanner::scan_root;
use lumenkorn_core::thumbs::{ThumbCache, Variant, generate_pending};
use lumenkorn_core::*;
use std::path::Path;
use std::sync::Mutex;

fn write_jpeg(path: &Path, w: u32, h: u32, seed: u8) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let img = RgbImage::from_fn(w, h, |x, y| Rgb([seed, (x % 255) as u8, (y % 255) as u8]));
    img.save(path).unwrap();
}

fn no_progress(_: ScanProgress) {}

fn names(cat: &Catalog, scope: Scope) -> Vec<String> {
    let mut q = ImageQuery::new(scope);
    q.sort = SortOrder::NameAsc;
    cat.query_images(&q)
        .unwrap()
        .into_iter()
        .map(|i| i.file_name)
        .collect()
}

#[test]
fn scan_tree_projects_and_moves() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("Fotos");
    let b = tmp.path().join("Externe Platte");
    write_jpeg(&a.join("2024/Urlaub/strand.jpg"), 320, 200, 1);
    write_jpeg(&a.join("2024/Urlaub/berge.jpg"), 320, 200, 2);
    write_jpeg(&a.join("2024/Hochzeit/kirche.jpg"), 200, 320, 3);
    write_jpeg(&a.join("2025/schnee.png"), 100, 100, 4);
    write_jpeg(&b.join("Kunde X/shooting_01.jpg"), 300, 300, 5);
    std::fs::write(a.join("2024/notizen.txt"), "kein Bild").unwrap();
    write_jpeg(&a.join(".versteckt/geheim.jpg"), 50, 50, 6);

    let cat = Mutex::new(Catalog::open(&tmp.path().join("katalog.db")).unwrap());
    let (ra, rb) = {
        let c = cat.lock().unwrap();
        (c.add_root(&a).unwrap(), c.add_root(&b).unwrap())
    };

    // Verschachtelte Quellordner werden abgelehnt.
    assert!(cat.lock().unwrap().add_root(&a.join("2024")).is_err());

    let rep = scan_root(&cat, ra.id, &no_progress).unwrap();
    assert_eq!(rep.added, 4);
    let rep_b = scan_root(&cat, rb.id, &no_progress).unwrap();
    assert_eq!(rep_b.added, 1);

    // Zweiter Scan ohne Änderungen
    let rep = scan_root(&cat, ra.id, &no_progress).unwrap();
    assert_eq!((rep.added, rep.unchanged), (0, 4));

    let c = cat.lock().unwrap();

    // Ordnerbaum
    let tree = c.folder_tree(ra.id).unwrap();
    assert_eq!(tree.total, 4);
    let y2024 = tree.children.iter().find(|n| n.name == "2024").unwrap();
    assert_eq!((y2024.count, y2024.total), (0, 3));
    let child_names: Vec<_> = y2024.children.iter().map(|n| n.name.as_str()).collect();
    assert_eq!(child_names, ["Hochzeit", "Urlaub"]);

    // Ordner-Ansicht mit und ohne Unterordner
    let p2024 = y2024.path.clone();
    assert_eq!(
        names(
            &c,
            Scope::Folder {
                path: p2024.clone(),
                recursive: true
            }
        )
        .len(),
        3
    );
    assert!(
        names(
            &c,
            Scope::Folder {
                path: p2024.clone(),
                recursive: false
            }
        )
        .is_empty()
    );

    // Projekt aus zwei Ordnern auf verschiedenen „Laufwerken“
    let p = c.create_project("Portfolio", "").unwrap();
    c.add_project_folder(p.id, &a.join("2024/Urlaub"), true)
        .unwrap();
    c.add_project_folder(p.id, &b.join("Kunde X"), true)
        .unwrap();
    // Überlappender Ordner: Bilder dürfen nicht doppelt erscheinen
    c.add_project_folder(p.id, &a.join("2024"), false).unwrap();
    let p = c.project(p.id).unwrap();
    assert_eq!(p.folders.len(), 3);
    assert_eq!(p.image_count, 3);
    assert_eq!(
        names(&c, Scope::Project { id: p.id }),
        ["berge.jpg", "shooting_01.jpg", "strand.jpg"]
    );

    // Bewertung, Schlagworte, Bearbeitung an „strand.jpg“ hängen
    let strand = c
        .query_images(&ImageQuery {
            search: Some("strand".into()),
            ..ImageQuery::new(Scope::All)
        })
        .unwrap()[0]
        .id;
    c.set_rating(&[strand], 4).unwrap();
    c.add_tag(strand, "Meer", TagSource::Ai, Some(0.91))
        .unwrap();
    c.add_tag(strand, "Sonnenuntergang", TagSource::Manual, None)
        .unwrap();
    // KI darf manuelles Schlagwort nicht überschreiben
    c.add_tag(strand, "Sonnenuntergang", TagSource::Ai, Some(0.4))
        .unwrap();
    drop(c);
    cat.lock()
        .unwrap()
        .save_develop(
            &[strand],
            &lumenkorn_develop::DevelopParams {
                exposure: 0.3,
                ..Default::default()
            },
        )
        .unwrap();

    // Suche über Schlagworte (mehrere Wörter = UND)
    {
        let c = cat.lock().unwrap();
        let hits = c
            .query_images(&ImageQuery {
                search: Some("meer sonnen".into()),
                ..ImageQuery::new(Scope::All)
            })
            .unwrap();
        assert_eq!(hits.len(), 1);
        let tags = c.image(strand).unwrap().tags;
        assert_eq!(
            tags.iter()
                .find(|t| t.name == "Sonnenuntergang")
                .unwrap()
                .source,
            TagSource::Manual
        );
    }

    // Datei außerhalb der App umbenennen und verschieben
    std::fs::create_dir_all(a.join("Archiv")).unwrap();
    std::fs::rename(
        a.join("2024/Urlaub/strand.jpg"),
        a.join("Archiv/strand_final.jpg"),
    )
    .unwrap();
    std::fs::remove_file(a.join("2025/schnee.png")).unwrap();
    write_jpeg(&a.join("2025/neu.jpg"), 120, 80, 9);

    let rep = scan_root(&cat, ra.id, &no_progress).unwrap();
    assert_eq!((rep.moved, rep.missing, rep.added), (1, 1, 1));

    let c = cat.lock().unwrap();
    let moved = c.image(strand).unwrap();
    assert!(
        moved.path.ends_with("strand_final.jpg"),
        "gleiche ID, neuer Pfad"
    );
    assert_eq!(moved.rating, 4);
    assert_eq!(moved.tags.len(), 2);
    assert_eq!(
        c.current_edit(strand).unwrap().unwrap().params["exposure"],
        0.3
    );

    // Fehlende Bilder erscheinen nicht mehr
    assert!(!names(&c, Scope::All).contains(&"schnee.png".to_string()));

    // Quellordner entfernen lässt Dateien unberührt
    c.remove_root(rb.id).unwrap();
    assert!(b.join("Kunde X/shooting_01.jpg").exists());
}

#[test]
fn generates_thumbnails() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("Fotos");
    write_jpeg(&a.join("gross.jpg"), 2000, 1000, 1);
    write_jpeg(&a.join("klein.jpg"), 200, 100, 2);
    std::fs::write(a.join("kaputt.jpg"), b"\xFF\xD8\xFF kein echtes jpeg").unwrap();

    let cat = Mutex::new(Catalog::open_in_memory().unwrap());
    let root = cat.lock().unwrap().add_root(&a).unwrap();
    scan_root(&cat, root.id, &no_progress).unwrap();

    let cache = ThumbCache::new(tmp.path().join("cache"));
    let done = Mutex::new(Vec::new());
    let n = generate_pending(&cat, &cache, &|id, ok| done.lock().unwrap().push((id, ok))).unwrap();
    assert_eq!(n, 3);
    assert_eq!(done.lock().unwrap().iter().filter(|(_, ok)| *ok).count(), 2);

    let imgs = cat
        .lock()
        .unwrap()
        .query_images(&ImageQuery::new(Scope::All))
        .unwrap();
    let gross = imgs.iter().find(|i| i.file_name == "gross.jpg").unwrap();
    assert!(gross.thumb_ready);
    let thumb = image::load_from_memory(&cache.read(Variant::Thumb, gross.id).unwrap()).unwrap();
    assert_eq!((thumb.width(), thumb.height()), (512, 256));

    // Fehlgeschlagene werden nicht endlos wiederholt
    assert_eq!(generate_pending(&cat, &cache, &|_, _| {}).unwrap(), 0);
}

#[test]
fn reopening_keeps_data() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("k.db");
    {
        let c = Catalog::open(&db).unwrap();
        c.create_project("Bleibt", "").unwrap();
    }
    let c = Catalog::open(&db).unwrap();
    assert_eq!(c.projects().unwrap()[0].name, "Bleibt");
}

#[test]
fn edits_drive_thumbnails() {
    use lumenkorn_develop::DevelopParams;

    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("Fotos");
    write_jpeg(&a.join("grau.jpg"), 400, 300, 128);

    let cat = Mutex::new(Catalog::open_in_memory().unwrap());
    let root = cat.lock().unwrap().add_root(&a).unwrap();
    scan_root(&cat, root.id, &no_progress).unwrap();
    let cache = ThumbCache::new(tmp.path().join("cache"));
    generate_pending(&cat, &cache, &|_, _| {}).unwrap();

    let id = cat
        .lock()
        .unwrap()
        .query_images(&ImageQuery::new(Scope::All))
        .unwrap()[0]
        .id;
    let brightness = |bytes: Vec<u8>| -> f32 {
        let img = image::load_from_memory(&bytes).unwrap().to_luma8();
        img.pixels().map(|p| p[0] as f32).sum::<f32>() / img.len() as f32
    };
    let before = brightness(cache.read(Variant::Thumb, id).unwrap());

    // Belichtung +1: Thumbnail wird neu erzeugt und ist heller
    let p = DevelopParams {
        exposure: 1.0,
        ..Default::default()
    };
    cat.lock().unwrap().save_develop(&[id], &p).unwrap();
    assert!(
        cat.lock()
            .unwrap()
            .query_images(&ImageQuery::new(Scope::All))
            .unwrap()[0]
            .edited
    );
    assert_eq!(generate_pending(&cat, &cache, &|_, _| {}).unwrap(), 1);
    let after = brightness(cache.read(Variant::Thumb, id).unwrap());
    assert!(after > before + 15.0, "{before} -> {after}");
    assert_eq!(
        cat.lock().unwrap().develop_params(id).unwrap().exposure,
        1.0
    );

    // Snapshot anlegen, zurücksetzen, Snapshot wiederherstellen
    let snap = cat.lock().unwrap().create_snapshot(id, "Hell").unwrap();
    cat.lock()
        .unwrap()
        .save_develop(&[id], &DevelopParams::default())
        .unwrap();
    {
        let c = cat.lock().unwrap();
        assert!(!c.query_images(&ImageQuery::new(Scope::All)).unwrap()[0].edited);
        assert_eq!(
            c.edits(id).unwrap().len(),
            1,
            "Snapshot bleibt beim Zurücksetzen erhalten"
        );
    }
    let restored = cat.lock().unwrap().restore_snapshot(snap.id).unwrap();
    assert_eq!(restored.exposure, 1.0);
    assert_eq!(
        cat.lock().unwrap().develop_params(id).unwrap().exposure,
        1.0
    );
}

#[test]
fn ai_results_replace_ai_tags_but_keep_manual_ones() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("Fotos");
    write_jpeg(&a.join("x.jpg"), 64, 64, 1);
    write_jpeg(&a.join("y.jpg"), 64, 64, 2);
    let cat = Mutex::new(Catalog::open_in_memory().unwrap());
    let root = cat.lock().unwrap().add_root(&a).unwrap();
    scan_root(&cat, root.id, &no_progress).unwrap();
    let cache = ThumbCache::new(tmp.path().join("cache"));
    generate_pending(&cat, &cache, &|_, _| {}).unwrap();

    let mut c = cat.lock().unwrap();
    assert_eq!(c.ai_counts().unwrap(), (0, 2));
    let ids = c.pending_ai(10).unwrap();
    assert_eq!(ids.len(), 2);
    let id = ids[0];
    c.add_tag(id, "Strand", TagSource::Manual, None).unwrap();

    c.save_ai_result(
        id,
        "m",
        &[1, 2, 3, 4],
        &[("Strand".into(), 0.9), ("Meer".into(), 0.7)],
    )
    .unwrap();
    let tags = c.image_tags(id).unwrap();
    assert_eq!(
        tags.iter().find(|t| t.name == "Strand").unwrap().source,
        TagSource::Manual
    );
    assert!(
        tags.iter()
            .any(|t| t.name == "Meer" && t.source == TagSource::Ai)
    );

    // Neuer Lauf ersetzt KI-Schlagworte
    c.save_ai_result(id, "m", &[5, 6, 7, 8], &[("Berge".into(), 0.8)])
        .unwrap();
    let names: Vec<String> = c
        .image_tags(id)
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert!(
        names.contains(&"Berge".to_string())
            && names.contains(&"Strand".to_string())
            && !names.contains(&"Meer".to_string())
    );
    assert_eq!(c.embeddings("m").unwrap(), vec![(id, vec![5, 6, 7, 8])]);
    assert_eq!(c.ai_counts().unwrap(), (1, 2));
    assert_eq!(c.pending_ai(10).unwrap().len(), 1);

    // Ähnlichkeitsreihenfolge bleibt erhalten und respektiert den Bereich
    let other = ids[1];
    let ordered = c
        .images_in_order(&[other, id, 9999], &Scope::All, None)
        .unwrap();
    assert_eq!(
        ordered.iter().map(|i| i.id).collect::<Vec<_>>(),
        vec![other, id]
    );

    c.reset_ai().unwrap();
    assert_eq!(c.pending_ai(10).unwrap().len(), 2);
}

/// Abgleich nur der gemeldeten Pfade (Dateiüberwachung) statt des ganzen Ordners.
#[test]
fn scan_paths_updates_only_changed_parts() {
    use lumenkorn_core::scanner::scan_paths;
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("Fotos");
    write_jpeg(&a.join("Reise/eins.jpg"), 200, 100, 1);
    write_jpeg(&a.join("Reise/zwei.jpg"), 200, 100, 2);
    write_jpeg(&a.join("Andere/drei.jpg"), 200, 100, 3);
    let cat = Mutex::new(Catalog::open(&tmp.path().join("katalog.db")).unwrap());
    let root = cat.lock().unwrap().add_root(&a).unwrap();
    scan_root(&cat, root.id, &no_progress).unwrap();
    let scan =
        |paths: &[std::path::PathBuf]| scan_paths(&cat, root.id, paths, &no_progress).unwrap();

    // Neue Datei: nur sie wird angefasst, der Rest zählt nicht einmal als unverändert
    write_jpeg(&a.join("Reise/vier.jpg"), 200, 100, 4);
    let rep = scan(&[a.join("Reise/vier.jpg")]);
    assert_eq!((rep.added, rep.unchanged, rep.missing), (1, 0, 0));

    // Datei geändert
    std::thread::sleep(std::time::Duration::from_millis(1100)); // mtime in Sekunden
    write_jpeg(&a.join("Reise/eins.jpg"), 300, 100, 9);
    assert_eq!(scan(&[a.join("Reise/eins.jpg")]).updated, 1);

    // Datei gelöscht → fehlend; Überwachung meldet nur den Pfad
    std::fs::remove_file(a.join("Andere/drei.jpg")).unwrap();
    assert_eq!(scan(&[a.join("Andere/drei.jpg")]).missing, 1);
    assert!(!names(&cat.lock().unwrap(), Scope::All).contains(&"drei.jpg".to_string()));

    // Ordner umbenannt: alter und neuer Pfad gemeldet → als verschoben erkannt
    let edited = {
        let c = cat.lock().unwrap();
        let id = c
            .query_images(&ImageQuery::new(Scope::All))
            .unwrap()
            .into_iter()
            .find(|i| i.file_name == "zwei.jpg")
            .unwrap()
            .id;
        c.set_rating(&[id], 5).unwrap();
        id
    };
    std::fs::rename(a.join("Reise"), a.join("Reise 2026")).unwrap();
    let rep = scan(&[a.join("Reise"), a.join("Reise 2026")]);
    assert_eq!((rep.moved, rep.missing, rep.added), (3, 0, 0));
    let c = cat.lock().unwrap();
    let moved = c
        .query_images(&ImageQuery::new(Scope::All))
        .unwrap()
        .into_iter()
        .find(|i| i.id == edited)
        .unwrap();
    assert!(moved.path.contains("Reise 2026"), "gleiche ID, neuer Pfad");
    assert_eq!(moved.rating, 5, "Bewertung bleibt");
    drop(c);

    // Gelöschte Datei taucht wieder auf
    write_jpeg(&a.join("Andere/drei.jpg"), 200, 100, 3);
    let rep = scan(&[a.join("Andere")]);
    assert_eq!(rep.added + rep.updated + rep.moved, 1);
    assert!(names(&cat.lock().unwrap(), Scope::All).contains(&"drei.jpg".to_string()));

    // Versteckte Ordner, fremde Pfade und Nicht-Bilder werden ignoriert
    write_jpeg(&a.join(".cache/x.jpg"), 50, 50, 7);
    std::fs::write(a.join("Andere/drei.xmp"), "<xmp/>").unwrap();
    let rep = scan(&[
        a.join(".cache/x.jpg"),
        a.join("Andere/drei.xmp"),
        tmp.path().join("woanders.jpg"),
    ]);
    assert_eq!(
        rep,
        ScanReport {
            root_id: root.id,
            ..Default::default()
        }
    );
    assert_eq!(names(&cat.lock().unwrap(), Scope::All).len(), 4);
}

/// Ein bereits fehlendes Foto, das später an anderer Stelle auftaucht, wird
/// wiedererkannt – auch beim Teil-Abgleich der Dateiüberwachung.
#[test]
fn scan_paths_relinks_previously_missing_photos() {
    use lumenkorn_core::scanner::scan_paths;
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("Fotos");
    write_jpeg(&a.join("Reise/eins.jpg"), 200, 100, 1);
    let cat = Mutex::new(Catalog::open(&tmp.path().join("katalog.db")).unwrap());
    let root = cat.lock().unwrap().add_root(&a).unwrap();
    scan_root(&cat, root.id, &no_progress).unwrap();
    let id = cat
        .lock()
        .unwrap()
        .query_images(&ImageQuery::new(Scope::All))
        .unwrap()[0]
        .id;
    cat.lock().unwrap().set_rating(&[id], 5).unwrap();

    // Ordner aus dem Quellordner hinaus → fehlt
    std::fs::rename(a.join("Reise"), tmp.path().join("weg")).unwrap();
    assert_eq!(
        scan_paths(&cat, root.id, &[a.join("Reise")], &no_progress)
            .unwrap()
            .missing,
        1
    );

    // … und unter neuem Namen zurück
    std::fs::rename(tmp.path().join("weg"), a.join("Reise 2")).unwrap();
    let rep = scan_paths(&cat, root.id, &[a.join("Reise 2")], &no_progress).unwrap();
    assert_eq!((rep.moved, rep.added), (1, 0));
    let imgs = cat
        .lock()
        .unwrap()
        .query_images(&ImageQuery::new(Scope::All))
        .unwrap();
    assert_eq!(imgs.len(), 1);
    assert_eq!((imgs[0].id, imgs[0].rating), (id, 5));
}

#[test]
fn merge_result_joins_catalog() {
    use lumenkorn_core::scanner::scan_paths;
    use lumenkorn_develop::{ImageF32, exr_io};

    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("Fotos");
    write_jpeg(&a.join("reihe_1.jpg"), 640, 400, 1);
    let cat = Mutex::new(Catalog::open_in_memory().unwrap());
    let root = cat.lock().unwrap().add_root(&a).unwrap();
    scan_root(&cat, root.id, &no_progress).unwrap();
    let src = cat
        .lock()
        .unwrap()
        .query_images(&ImageQuery::new(Scope::All))
        .unwrap()[0]
        .clone();
    assert_eq!(cat.lock().unwrap().image_root(src.id).unwrap(), root.id);

    // Ergebnis mit Lichtern über 1,0 neben die Quelle schreiben und aufnehmen
    let img = ImageF32::from_fn(900, 300, |x, _| [x as f32 / 300.0, 0.5, 0.25]);
    let out = a.join("reihe_1-Pano.exr");
    exr_io::write(&img, &out, false).unwrap();
    let rep = scan_paths(&cat, root.id, std::slice::from_ref(&out), &no_progress).unwrap();
    assert_eq!(rep.added, 1);
    let id = cat
        .lock()
        .unwrap()
        .adopt_capture_info(&out.to_string_lossy(), src.id)
        .unwrap()
        .expect("EXR im Katalog");

    let imgs = cat
        .lock()
        .unwrap()
        .query_images(&ImageQuery::new(Scope::All))
        .unwrap();
    let merged = imgs.iter().find(|i| i.id == id).unwrap();
    assert_eq!((merged.width, merged.height), (Some(900), Some(300)));
    assert_eq!(
        merged.captured_at, src.captured_at,
        "Aufnahmedatum übernommen"
    );

    // Vorschaubild aus der linearen Datei
    let cache = ThumbCache::new(tmp.path().join("cache"));
    generate_pending(&cat, &cache, &|_, _| {}).unwrap();
    let thumb = image::load_from_memory(&cache.read(Variant::Thumb, id).unwrap()).unwrap();
    assert_eq!((thumb.width(), thumb.height()), (512, 171));
}

#[test]
fn deleted_ids_are_never_reused() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("Fotos");
    write_jpeg(&a.join("a.jpg"), 64, 64, 1);
    write_jpeg(&a.join("b.jpg"), 64, 64, 2);
    let cat = Mutex::new(Catalog::open(&tmp.path().join("k.db")).unwrap());
    let root = cat.lock().unwrap().add_root(&a).unwrap();
    scan_root(&cat, root.id, &no_progress).unwrap();
    let max_id = |c: &Catalog| {
        c.query_images(&ImageQuery::new(Scope::All))
            .unwrap()
            .iter()
            .map(|i| i.id)
            .max()
            .unwrap()
    };
    let newest = max_id(&cat.lock().unwrap());
    // Neuestes Foto löschen (wie „Abgelehnte löschen“), dann ein neues aufnehmen
    std::fs::remove_file(a.join("b.jpg")).ok();
    std::fs::remove_file(a.join("a.jpg")).ok();
    cat.lock().unwrap().forget_images(&[newest]).unwrap();
    write_jpeg(&a.join("c.jpg"), 64, 64, 3);
    scan_root(&cat, root.id, &no_progress).unwrap();
    let c = cat.lock().unwrap();
    let new_id = c
        .query_images(&ImageQuery::new(Scope::All))
        .unwrap()
        .into_iter()
        .find(|i| i.file_name == "c.jpg")
        .unwrap()
        .id;
    assert!(
        new_id > newest,
        "ID {new_id} wiederverwendet (gelöscht: {newest})"
    );
}

#[test]
fn new_and_empty_folders_appear_in_tree() {
    use lumenkorn_core::scanner::scan_paths;
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("Fotos");
    write_jpeg(&a.join("2025/alt.jpg"), 64, 64, 1);
    let cat = Mutex::new(Catalog::open_in_memory().unwrap());
    let root = cat.lock().unwrap().add_root(&a).unwrap();
    scan_root(&cat, root.id, &no_progress).unwrap();
    let names = |c: &Catalog| -> Vec<String> {
        fn walk(n: &FolderNode, out: &mut Vec<String>) {
            for c in &n.children {
                out.push(format!("{}:{}", c.name, c.total));
                walk(c, out);
            }
        }
        let mut v = Vec::new();
        walk(&c.folder_tree(root.id).unwrap(), &mut v);
        v
    };

    // Neuer, leerer Ordner und einer nur mit HEIC-Dateien (Handy) – über die
    // Dateiüberwachung gemeldet
    let leer = a.join("Neu angelegt");
    std::fs::create_dir_all(&leer).unwrap();
    let handy = a.join("Handy");
    std::fs::create_dir_all(&handy).unwrap();
    std::fs::write(handy.join("IMG_0001.HEIC"), b"x").unwrap();
    std::fs::write(handy.join("IMG_0002.heic"), b"x").unwrap();
    let rep = scan_paths(&cat, root.id, &[leer.clone(), handy.clone()], &no_progress).unwrap();
    assert!(rep.folders_changed);
    assert_eq!(rep.unsupported, vec![("heic".to_string(), 2)]);
    let t = names(&cat.lock().unwrap());
    assert!(t.contains(&"Neu angelegt:0".to_string()), "{t:?}");
    assert!(t.contains(&"Handy:0".to_string()), "{t:?}");

    // Fotos in einen neuen Unterordner: beim vollständigen Neu-Einlesen gefunden
    write_jpeg(&a.join("2025/Urlaub/neu.jpg"), 64, 64, 2);
    let rep = scan_root(&cat, root.id, &no_progress).unwrap();
    assert_eq!(rep.added, 1);
    let t = names(&cat.lock().unwrap());
    assert!(t.contains(&"Urlaub:1".to_string()), "{t:?}");

    // Gelöschter Ordner verschwindet aus dem Baum
    std::fs::remove_dir_all(&leer).unwrap();
    let rep = scan_paths(&cat, root.id, &[leer], &no_progress).unwrap();
    assert!(rep.folders_changed);
    let t = names(&cat.lock().unwrap());
    assert!(!t.iter().any(|n| n.starts_with("Neu angelegt")), "{t:?}");
    // Erneuter vollständiger Scan ohne Änderungen meldet keine Ordneränderung
    let rep = scan_root(&cat, root.id, &no_progress).unwrap();
    assert!(!rep.folders_changed);
}
