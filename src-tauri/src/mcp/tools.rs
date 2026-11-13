//! Die Werkzeuge: Beschreibung (`*_tool`) und Ausführung je Werkzeug, dazu die
//! Prüfung von Exportordnern.

use super::*;

/// Höchstzahl Fotos je Export-Aufruf
pub(super) const MAX_EXPORT: usize = 500;

/// Pfad als Bestandteile: ohne `\\?\`-Präfix, `.` und `..` aufgelöst, unter
/// Windows ohne Groß/Klein-Unterschied.
pub(super) fn path_parts(p: &str) -> Vec<String> {
    let p = p.strip_prefix(r"\\?\").unwrap_or(p);
    let mut out: Vec<String> = Vec::new();
    for part in p.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s if cfg!(windows) => out.push(s.to_lowercase()),
            s => out.push(s.to_string()),
        }
    }
    out
}

/// Liegt `child` in `parent` (oder ist es selbst)? Komponentenweise, damit
/// `C:\Bilder2` nicht als in `C:\Bilder` gilt.
pub(super) fn path_inside(child: &str, parent: &str) -> bool {
    let (c, p) = (path_parts(child), path_parts(parent));
    !p.is_empty() && c.starts_with(&p)
}

/// Prüft den Zielordner eines Exports: absolut, vorhanden und (nach
/// Auflösen von Verknüpfungen) innerhalb eines der erlaubten Ordner.
pub(super) fn check_export_folder(
    folder: &str,
    allowed: &[std::path::PathBuf],
) -> Result<String, String> {
    const MSG: &str =
        "folder must be inside one of the library folders; omit it to export next to the originals";
    let path = std::path::Path::new(folder);
    if !path.is_absolute() {
        return Err(format!("folder must be an absolute path; {MSG}"));
    }
    let real = std::fs::canonicalize(path).map_err(|_| format!("folder does not exist; {MSG}"))?;
    if !real.is_dir() {
        return Err(format!("folder is not a directory; {MSG}"));
    }
    let real = real.to_string_lossy().into_owned();
    let ok = allowed.iter().any(|a| {
        let a = std::fs::canonicalize(a).unwrap_or_else(|_| a.clone());
        path_inside(&real, &a.to_string_lossy())
    });
    if ok {
        Ok(real.strip_prefix(r"\\?\").unwrap_or(&real).to_string())
    } else {
        Err(MSG.into())
    }
}

pub(super) fn tool(
    name: &str,
    title: &str,
    description: &str,
    schema: Value,
    read_only: bool,
) -> Value {
    // Ersetzt vorhandene Bearbeitungen bzw. (Export „overwrite“) Dateien. Bearbeitungen
    // kommen vorher in einen Schnappschuss, sind aber ohne Zutun des Nutzers weg.
    let destructive = matches!(
        name,
        "set_develop_settings"
            | "auto_tone"
            | "reset_photos"
            | "apply_preset"
            | "copy_settings"
            | "export_photos"
    );
    json!({
        "name": name,
        "title": title,
        "description": description,
        "inputSchema": schema,
        "annotations": { "readOnlyHint": read_only, "destructiveHint": destructive, "openWorldHint": false },
    })
}

pub(super) fn ids_schema() -> Value {
    json!({ "type": "array", "items": { "type": "integer" }, "minItems": 1, "description": "Photo ids" })
}

pub(super) fn tools() -> Vec<Value> {
    vec![
        list_library_tool(),
        search_photos_tool(),
        get_photo_tool(),
        view_photo_tool(),
        rate_photos_tool(),
        flag_photos_tool(),
        tag_photos_tool(),
        edit_metadata_tool(),
        get_develop_settings_tool(),
        set_develop_settings_tool(),
        auto_tone_tool(),
        reset_photos_tool(),
        apply_preset_tool(),
        copy_settings_tool(),
        add_mask_tool(),
        export_photos_tool(),
        show_photo_tool(),
    ]
}

pub(super) type ToolResult = Result<Vec<Value>, String>;

pub(super) fn text(v: impl Serialize) -> ToolResult {
    Ok(vec![json!({
        "type": "text",
        "text": serde_json::to_string_pretty(&v).unwrap_or_default(),
    })])
}

pub(super) fn arg<'a>(a: &'a Value, k: &str) -> Option<&'a Value> {
    a.get(k).filter(|v| !v.is_null())
}
pub(super) fn int(a: &Value, k: &str) -> Result<i64, String> {
    arg(a, k)
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("`{k}` (integer) is required"))
}
pub(super) fn ids(a: &Value, k: &str) -> Result<Vec<i64>, String> {
    let v: Vec<i64> = arg(a, k)
        .and_then(Value::as_array)
        .map(|l| l.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default();
    if v.is_empty() {
        return Err(format!("`{k}` (list of photo ids) is required"));
    }
    Ok(v)
}
pub(super) fn e(err: Error) -> String {
    err.to_string()
}

/// Bevor ein Werkzeug Bearbeitungen ändert: Schnappschuss der jetzigen, damit sie
/// sich im Entwickeln wiederherstellen lassen (Rückgängig gibt es nur für das
/// geöffnete Foto).
pub(super) fn keep_edits(app: &AppHandle, ids: &[i64]) -> Result<(), String> {
    let name =
        lumenkorn_develop::i18n::tr_s("Vor Änderung durch KI-Agent", "Before change by AI agent");
    app.state::<AppState>()
        .catalog
        .locked()
        .snapshot_unsaved(ids, name)
        .map(|_| ())
        .map_err(e)
}

/// Oberfläche über Änderungen informieren
pub(super) fn changed(app: &AppHandle, develop: &[i64]) {
    let _ = app.emit("catalog-updated", ());
    if !develop.is_empty() {
        let _ = app.emit("develop-external", json!({ "ids": develop }));
    }
}

pub(super) async fn call(app: &AppHandle, name: &str, a: &Value) -> ToolResult {
    log::info!("MCP: {name}");
    match name {
        "list_library" => list_library(app, a).await,
        "search_photos" => search(app, a).await,
        "get_photo" => get_photo(app, a).await,
        "view_photo" => view_photo(app, a).await,
        "rate_photos" => rate_photos(app, a).await,
        "flag_photos" => flag_photos(app, a).await,
        "tag_photos" => tag_photos(app, a).await,
        "edit_metadata" => edit_metadata(app, a).await,
        "get_develop_settings" => get_develop_settings(app, a).await,
        "set_develop_settings" => set_develop_settings(app, a).await,
        "auto_tone" => auto_tone(app, a).await,
        "reset_photos" => reset_photos(app, a).await,
        "apply_preset" => apply_preset(app, a).await,
        "copy_settings" => copy_settings(app, a).await,
        "add_mask" => add_mask(app, a).await,
        "export_photos" => export_photos(app, a).await,
        "show_photo" => show_photo(app, a).await,
        _ => Err(format!("unknown tool: {name}")),
    }
}

pub(super) fn list_library_tool() -> Value {
    tool(
        "list_library",
        "List folders, projects and people",
        "Source folders (with photo counts per subfolder), projects, named people and people groups, and presets. Use the ids and paths for search_photos.",
        json!({ "type": "object", "properties": {
            "max_folders": { "type": "integer", "description": "Maximum number of folders listed (default 300)" }
        } }),
        true,
    )
}

pub(super) async fn list_library(app: &AppHandle, a: &Value) -> ToolResult {
    let state = app.state::<AppState>();
    let max = arg(a, "max_folders").and_then(Value::as_u64).unwrap_or(300) as usize;
    let roots = crate::commands::list_roots(state.clone()).map_err(e)?;
    let mut folders = Vec::new();
    for r in &roots {
        if let Ok(tree) = crate::commands::folder_tree(state.clone(), r.id) {
            flatten(&tree, 0, &mut folders, max);
        }
    }
    let projects = crate::commands::list_projects(state.clone()).map_err(e)?;
    let people = crate::people::people_list(state.clone()).map_err(e)?;
    let groups = crate::people::person_groups(state.clone()).map_err(e)?;
    let presets = crate::develop::preset_list(state.clone()).map_err(e)?;
    text(json!({
        "folders": folders,
        "projects": projects.iter().map(|p| json!({ "id": p.id, "name": p.name })).collect::<Vec<_>>(),
        "people": people.iter().filter(|p| p.name.is_some()).map(|p| json!({
            "id": p.id, "name": p.name, "photos": p.image_count,
            "group": p.group_id.and_then(|g| groups.iter().find(|x| x.id == g)).map(|g| g.name.clone()),
        })).collect::<Vec<_>>(),
        "unnamedPeopleGroups": people.iter().filter(|p| p.name.is_none()).count(),
        "presets": presets.iter().map(|p| json!({ "id": p.id, "name": p.name })).collect::<Vec<_>>(),
    }))
}

pub(super) fn search_photos_tool() -> Value {
    tool(
        "search_photos",
        "Search photos",
        "Find photos. All filters are optional and combined. `text` searches file name, folder, camera, lens, keywords and people; with `by_content` it searches what is in the picture (AI, e.g. \"sunset over the sea\"). Returns id, file, date, rating (0–5), flag (1 pick, -1 rejected, 0 none) and whether it is edited.",
        json!({ "type": "object", "properties": {
            "text": { "type": "string" },
            "by_content": { "type": "boolean", "description": "Search by image content (needs the AI keyword model)" },
            "folder": { "type": "string", "description": "Folder path from list_library" },
            "include_subfolders": { "type": "boolean", "default": true },
            "project_id": { "type": "integer" },
            "person_id": { "type": "integer" },
            "min_rating": { "type": "integer", "minimum": 0, "maximum": 5 },
            "file_types": { "type": "array", "items": { "type": "string" }, "description": "Only these file types, e.g. [\"arw\", \"jpeg\"]; \"raw\" means every RAW format" },
            "flag": { "type": "string", "enum": ["pick", "rejected", "none"] },
            "edited": { "type": "boolean" },
            "sort": { "type": "string", "enum": ["oldest", "newest", "name", "rating"], "default": "oldest" },
            "limit": { "type": "integer", "default": 50, "maximum": 500 },
            "offset": { "type": "integer", "default": 0 }
        } }),
        true,
    )
}

pub(super) fn get_photo_tool() -> Value {
    tool(
        "get_photo",
        "Photo details",
        "Metadata of one photo: file, folder, date, camera, lens, exposure, size, rating, flag, keywords, recognized people and the develop settings that differ from the defaults.",
        json!({ "type": "object", "properties": { "id": { "type": "integer" } }, "required": ["id"] }),
        true,
    )
}

pub(super) async fn get_photo(app: &AppHandle, a: &Value) -> ToolResult {
    let state = app.state::<AppState>();
    let id = int(a, "id")?;
    let detail = crate::commands::image_detail(state.clone(), id).map_err(e)?;
    let faces = crate::people::image_faces(state.clone(), id).unwrap_or_default();
    let params = crate::develop::develop_params(state.clone(), id).map_err(e)?;
    let mut v = serde_json::to_value(&detail).unwrap_or_default();
    v["people"] = json!(
        faces
            .iter()
            .filter_map(|f| f.person_name.clone())
            .collect::<Vec<_>>()
    );
    v["developChanges"] = non_default(&params);
    text(v)
}

pub(super) fn view_photo_tool() -> Value {
    tool(
        "view_photo",
        "Look at a photo",
        "Returns the photo as a JPEG image with its current edits, so you can judge it. `size` is the longest edge in pixels (default 1024, up to 2048).",
        json!({ "type": "object", "properties": {
            "id": { "type": "integer" },
            "size": { "type": "integer", "minimum": 128, "maximum": 2048, "default": 1024 }
        }, "required": ["id"] }),
        true,
    )
}

pub(super) async fn view_photo(app: &AppHandle, a: &Value) -> ToolResult {
    let state = app.state::<AppState>();
    let id = int(a, "id")?;
    let size = arg(a, "size")
        .and_then(Value::as_u64)
        .unwrap_or(1024)
        .clamp(128, 2048) as u32;
    let job = state.catalog.locked().thumb_job(id).map_err(e)?;
    let bytes = state
        .cache
        .get_or_render(
            &state.catalog,
            lumenkorn_core::thumbs::Variant::Preview,
            &job,
        )
        .map_err(e)?;
    let img = image::load_from_memory(&bytes).map_err(|x| x.to_string())?;
    let img = if img.width().max(img.height()) > size {
        img.resize(size, size, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let mut out = Vec::new();
    img.to_rgb8()
        .write_with_encoder(image::codecs::jpeg::JpegEncoder::new_with_quality(
            &mut out, 85,
        ))
        .map_err(|x| x.to_string())?;
    use base64::Engine;
    Ok(vec![
        json!({ "type": "image", "mimeType": "image/jpeg",
                    "data": base64::engine::general_purpose::STANDARD.encode(&out) }),
        json!({ "type": "text", "text": format!("Photo {id}: {} ({}×{} shown)", job.path, img.width(), img.height()) }),
    ])
}

pub(super) fn rate_photos_tool() -> Value {
    tool(
        "rate_photos",
        "Rate photos",
        "Set the star rating (0 removes it).",
        json!({ "type": "object", "properties": {
            "ids": ids_schema(),
            "rating": { "type": "integer", "minimum": 0, "maximum": 5 }
        }, "required": ["ids", "rating"] }),
        false,
    )
}

pub(super) async fn rate_photos(app: &AppHandle, a: &Value) -> ToolResult {
    let state = app.state::<AppState>();
    let ids = ids(a, "ids")?;
    let r = int(a, "rating")?.clamp(0, 5);
    // Nicht über `commands::set_rating`: Der Aussortier-Assistent lernt nur vom Nutzer
    state.catalog.locked().set_rating(&ids, r).map_err(e)?;
    changed(app, &[]);
    text(json!({ "rated": ids.len(), "rating": r }))
}

pub(super) fn flag_photos_tool() -> Value {
    tool(
        "flag_photos",
        "Flag photos",
        "Mark photos as pick, rejected, or remove the flag. Rejecting does not delete anything.",
        json!({ "type": "object", "properties": {
            "ids": ids_schema(),
            "flag": { "type": "string", "enum": ["pick", "rejected", "none"] }
        }, "required": ["ids", "flag"] }),
        false,
    )
}

pub(super) async fn flag_photos(app: &AppHandle, a: &Value) -> ToolResult {
    let state = app.state::<AppState>();
    let ids = ids(a, "ids")?;
    let f = match arg(a, "flag").and_then(Value::as_str) {
        Some("pick") => 1,
        Some("rejected") => -1,
        Some("none") => 0,
        _ => return Err("`flag` must be pick, rejected or none".into()),
    };
    // Nicht über `commands::set_flag`: Der Aussortier-Assistent lernt nur vom Nutzer
    state.catalog.locked().set_flag(&ids, f).map_err(e)?;
    changed(app, &[]);
    text(json!({ "flagged": ids.len() }))
}

pub(super) fn tag_photos_tool() -> Value {
    tool(
        "tag_photos",
        "Add or remove keywords",
        "Add and/or remove manual keywords on photos.",
        json!({ "type": "object", "properties": {
            "ids": ids_schema(),
            "add": { "type": "array", "items": { "type": "string" } },
            "remove": { "type": "array", "items": { "type": "string" } }
        }, "required": ["ids"] }),
        false,
    )
}

pub(super) async fn tag_photos(app: &AppHandle, a: &Value) -> ToolResult {
    let state = app.state::<AppState>();
    let ids = ids(a, "ids")?;
    let list = |k| -> Vec<String> {
        arg(a, k)
            .and_then(Value::as_array)
            .map(|l| {
                l.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let (add, remove) = (list("add"), list("remove"));
    for id in &ids {
        for t in &add {
            crate::commands::add_tag(state.clone(), *id, t.clone()).map_err(e)?;
        }
        for t in &remove {
            crate::commands::remove_tag(state.clone(), *id, t.clone()).map_err(e)?;
        }
    }
    let _ = app.emit("tags-changed", json!({ "ids": ids }));
    changed(app, &[]);
    text(json!({ "photos": ids.len(), "added": add, "removed": remove }))
}

pub(super) fn edit_metadata_tool() -> Value {
    tool(
        "edit_metadata",
        "Edit photo metadata",
        "Change title, description, creator, copyright, capture time (YYYY-MM-DDThh:mm:ss), camera, lens, ISO, exposure time (\"1/250\" or seconds), aperture and focal length (mm) of photos. Only the fields you pass are changed; null resets a field to the value in the file. `shift_seconds` moves the capture time instead. The values are stored in the catalog and in an XMP sidecar next to each photo (created if missing); the photo file itself is never modified.",
        json!({ "type": "object", "properties": {
            "ids": ids_schema(),
            "title": { "type": ["string", "null"] },
            "description": { "type": ["string", "null"] },
            "creator": { "type": ["string", "null"] },
            "copyright": { "type": ["string", "null"] },
            "captured_at": { "type": ["string", "null"] },
            "camera": { "type": ["string", "null"] },
            "lens": { "type": ["string", "null"] },
            "iso": { "type": ["integer", "null"] },
            "exposure": { "type": ["string", "null"] },
            "aperture": { "type": ["number", "null"] },
            "focal_length": { "type": ["number", "null"] },
            "shift_seconds": { "type": "integer" }
        }, "required": ["ids"] }),
        false,
    )
}

pub(super) async fn edit_metadata(app: &AppHandle, a: &Value) -> ToolResult {
    let state = app.state::<AppState>();
    let ids = ids(a, "ids")?;
    let change = if let Some(by) = arg(a, "shift_seconds").and_then(Value::as_i64) {
        crate::meta::Change::Shift(by)
    } else {
        // Eigene Namen (snake_case) auf das Patch-Format abbilden:
        // vorhanden = setzen, null = zurücksetzen, fehlend = unverändert
        let mut patch = serde_json::Map::new();
        for (k, camel) in [
            ("title", "title"),
            ("description", "description"),
            ("creator", "creator"),
            ("copyright", "copyright"),
            ("captured_at", "capturedAt"),
            ("camera", "camera"),
            ("lens", "lens"),
            ("iso", "iso"),
            ("exposure", "exposure"),
            ("aperture", "aperture"),
            ("focal_length", "focalLength"),
        ] {
            match a.get(k) {
                None => {}
                Some(Value::Null) => {
                    patch.insert(camel.into(), json!("reset"));
                }
                Some(v) => {
                    patch.insert(camel.into(), json!({ "set": v }));
                }
            }
        }
        let patch = serde_json::from_value(Value::Object(patch))
            .map_err(|x| format!("invalid metadata: {x}"))?;
        crate::meta::Change::Patch(patch)
    };
    let report = crate::meta::edit_metadata(&state, &ids, change).map_err(e)?;
    changed(app, &[]);
    text(report)
}

pub(super) fn get_develop_settings_tool() -> Value {
    tool(
        "get_develop_settings",
        "Read develop settings",
        "All develop settings of a photo as JSON (the format set_develop_settings accepts).",
        json!({ "type": "object", "properties": { "id": { "type": "integer" } }, "required": ["id"] }),
        true,
    )
}

pub(super) async fn get_develop_settings(app: &AppHandle, a: &Value) -> ToolResult {
    let state = app.state::<AppState>();
    let id = int(a, "id")?;
    text(crate::develop::develop_params(state.clone(), id).map_err(e)?)
}

pub(super) fn set_develop_settings_tool() -> Value {
    tool(
        "set_develop_settings",
        "Change develop settings",
        "Merge the given settings into the photos' current ones (JSON merge: only the keys you pass change; nested objects merge, arrays are replaced). Keys and ranges: kelvin (light color in Kelvin, 2000…25000, RAW files only; 0 = as shot), temperature, tint (−100…100, relative to the camera); exposure (−5…5 EV); contrast, highlights, shadows, whites, blacks, clarity, dehaze, vibrance, saturation (−100…100); sharpen (0…100); monochrome (bool); lensCorrection (bool); hsl {hue, saturation, luminance: 8 values each for red, orange, yellow, green, aqua, blue, purple, magenta, −100…100}; colorGrade {shadows, midtones, highlights, global: {hue 0…360, saturation 0…100, luminance −100…100}, blending, balance}; toneCurve {rgb, red, green, blue: points [[x, y], …] 0…1 on sRGB-encoded values, sorted by x, at most 16, [] = unchanged; preserveColors (bool, default true: the rgb curve acts on luminance)}; crop {x, y, w, h normalized 0…1} or null; straighten (−45…45°); perspectiveVertical (−100…100, positive widens the top: fixes converging verticals when shooting upwards), perspectiveHorizontal (−100…100, positive makes the right edge taller); rotate (quarter turns 0…3); flipHorizontal (bool). Unknown keys are reported and ignored.",
        json!({ "type": "object", "properties": {
            "ids": ids_schema(),
            "settings": { "type": "object", "additionalProperties": true }
        }, "required": ["ids", "settings"] }),
        false,
    )
}

pub(super) async fn set_develop_settings(app: &AppHandle, a: &Value) -> ToolResult {
    let state = app.state::<AppState>();
    let ids = ids(a, "ids")?;
    let patch = arg(a, "settings")
        .filter(|v| v.is_object())
        .ok_or("`settings` (object) is required")?;
    let known = serde_json::to_value(DevelopParams::default()).unwrap_or_default();
    keep_edits(app, &ids)?;
    let unknown: Vec<&String> = patch
        .as_object()
        .unwrap()
        .keys()
        .filter(|k| known.get(k.as_str()).is_none())
        .collect();
    for id in &ids {
        let cur = crate::develop::develop_params(state.clone(), *id).map_err(e)?;
        let mut v = serde_json::to_value(&cur).unwrap_or_default();
        merge(&mut v, patch);
        let p: DevelopParams = serde_json::from_value(v).map_err(|x| format!("settings: {x}"))?;
        crate::develop::develop_save(app.clone(), state.clone(), vec![*id], p.sanitized())
            .map_err(e)?;
    }
    changed(app, &ids);
    text(json!({ "updated": ids.len(), "ignoredUnknownKeys": unknown }))
}

pub(super) fn auto_tone_tool() -> Value {
    tool(
        "auto_tone",
        "Auto tone",
        "Like the Auto button: sets exposure, contrast, highlights, shadows, whites, blacks and vibrance for a balanced result. Returns the values.",
        json!({ "type": "object", "properties": { "ids": ids_schema() }, "required": ["ids"] }),
        false,
    )
}

pub(super) async fn auto_tone(app: &AppHandle, a: &Value) -> ToolResult {
    let state = app.state::<AppState>();
    let ids = ids(a, "ids")?;
    keep_edits(app, &ids)?;
    let mut out = Vec::new();
    for id in &ids {
        let mut p = crate::develop::develop_params(state.clone(), *id).map_err(e)?;
        let t = crate::develop::develop_auto(app.clone(), *id, p.clone())
            .await
            .map_err(e)?;
        t.apply(&mut p);
        crate::develop::develop_save(app.clone(), state.clone(), vec![*id], p).map_err(e)?;
        out.push(json!({ "id": id, "values": t }));
    }
    changed(app, &ids);
    text(out)
}

pub(super) fn reset_photos_tool() -> Value {
    tool(
        "reset_photos",
        "Reset to original",
        "Remove all edits of the photos (they look like the camera original again).",
        json!({ "type": "object", "properties": { "ids": ids_schema() }, "required": ["ids"] }),
        false,
    )
}

pub(super) async fn reset_photos(app: &AppHandle, a: &Value) -> ToolResult {
    let state = app.state::<AppState>();
    let ids = ids(a, "ids")?;
    keep_edits(app, &ids)?;
    crate::develop::develop_save(
        app.clone(),
        state.clone(),
        ids.clone(),
        DevelopParams::default(),
    )
    .map_err(e)?;
    changed(app, &ids);
    text(json!({ "reset": ids.len() }))
}

pub(super) fn apply_preset_tool() -> Value {
    tool(
        "apply_preset",
        "Apply a preset",
        "Apply a saved preset (name or id from list_library) to photos.",
        json!({ "type": "object", "properties": {
            "ids": ids_schema(),
            "preset": { "description": "Preset name or id", "type": ["string", "integer"] }
        }, "required": ["ids", "preset"] }),
        false,
    )
}

pub(super) async fn apply_preset(app: &AppHandle, a: &Value) -> ToolResult {
    let state = app.state::<AppState>();
    let ids = ids(a, "ids")?;
    let presets = crate::develop::preset_list(state.clone()).map_err(e)?;
    let want = arg(a, "preset").ok_or("`preset` is required")?;
    let p = presets
        .iter()
        .find(|p| {
            want.as_i64() == Some(p.id)
                || want
                    .as_str()
                    .is_some_and(|n| n.eq_ignore_ascii_case(&p.name))
        })
        .ok_or_else(|| {
            format!(
                "No preset {want}. Available: {}",
                presets
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
    keep_edits(app, &ids)?;
    let mut groups = p.groups;
    // `basic` würde alle feinen Gruppen einschließen
    groups.basic = false;
    crate::develop::develop_transfer(state.clone(), ids.clone(), p.params.clone(), groups)
        .map_err(e)?;
    changed(app, &ids);
    text(json!({ "preset": p.name, "applied": ids.len() }))
}

pub(super) fn copy_settings_tool() -> Value {
    tool(
        "copy_settings",
        "Copy develop settings",
        "Copy parts of one photo's edit to other photos. `groups` chooses what (default: everything except geometry): whiteBalance, light, color, hsl, colorGrade, details, masks, geometry.",
        json!({ "type": "object", "properties": {
            "from_id": { "type": "integer" },
            "to_ids": ids_schema(),
            "groups": { "type": "array", "items": { "type": "string", "enum": ["whiteBalance", "light", "color", "hsl", "colorGrade", "details", "masks", "geometry"] } }
        }, "required": ["from_id", "to_ids"] }),
        false,
    )
}

pub(super) async fn copy_settings(app: &AppHandle, a: &Value) -> ToolResult {
    let state = app.state::<AppState>();
    let from = int(a, "from_id")?;
    let to = ids(a, "to_ids")?;
    let src = crate::develop::develop_params(state.clone(), from).map_err(e)?;
    let mut g = lumenkorn_develop::TransferGroups {
        basic: false,
        ..Default::default()
    };
    let names: Vec<String> = arg(a, "groups")
        .and_then(Value::as_array)
        .map(|l| {
            l.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let all = names.is_empty();
    let on = |n: &str| all && n != "geometry" || names.iter().any(|x| x == n);
    g.white_balance = on("whiteBalance");
    g.light = on("light");
    g.color = on("color");
    g.hsl = on("hsl");
    g.color_grade = on("colorGrade");
    g.details = on("details");
    g.masks = on("masks");
    g.geometry = on("geometry");
    keep_edits(app, &to)?;
    crate::develop::develop_transfer(state.clone(), to.clone(), src, g).map_err(e)?;
    changed(app, &to);
    text(json!({ "copiedTo": to.len() }))
}

pub(super) fn add_mask_tool() -> Value {
    tool(
        "add_mask",
        "Add an AI mask with local adjustments",
        "Detect an area with AI and adjust only there. kind: subject, sky, person (whole person), person:face (facial skin), person:hair, person:eyes, person:iris, person:brows, person:lips, person:bodySkin, person:clothes, animal (all animals), animal:bird, animal:mammal, animal:reptile. adjust: exposure (−4…4 EV), contrast, highlights, shadows, clarity, temperature, tint, saturation (−100…100). `invert` adjusts everything except the area. Needs the AI masks to be installed in the app.",
        json!({ "type": "object", "properties": {
            "id": { "type": "integer" },
            "kind": { "type": "string" },
            "adjust": { "type": "object", "properties": {
                "exposure": { "type": "number" }, "contrast": { "type": "number" },
                "highlights": { "type": "number" }, "shadows": { "type": "number" },
                "clarity": { "type": "number" }, "temperature": { "type": "number" },
                "tint": { "type": "number" }, "saturation": { "type": "number" }
            } },
            "invert": { "type": "boolean" },
            "name": { "type": "string" }
        }, "required": ["id", "kind", "adjust"] }),
        false,
    )
}

pub(super) fn export_photos_tool() -> Value {
    tool(
        "export_photos",
        "Export photos",
        "Export developed photos in the background. Without `folder`, each goes into the export subfolder set in the app's settings (default \"Export\") next to its original. `folder` must be an existing absolute path inside one of the library folders (or the user's Pictures folder); at most 500 photos per call. format jpeg (quality 1–100), png, tiff or dng (slightly lossy linear DNG with all edits; with dng_original the unedited data for developing elsewhere); several formats at once with `formats`, e.g. [\"dng\", \"jpeg\"]; long_edge limits the size in pixels. on_conflict decides what happens when a file with the same name exists: rename (default, adds -2), overwrite or skip; originals in the library are never overwritten.",
        json!({ "type": "object", "properties": {
            "ids": ids_schema(),
            "folder": { "type": "string" },
            "format": { "type": "string", "enum": ["jpeg", "png", "tiff", "dng"], "default": "jpeg" },
            "formats": { "type": "array", "items": { "type": "string", "enum": ["jpeg", "png", "tiff", "dng"] } },
            "dng_original": { "type": "boolean", "default": false },
            "on_conflict": { "type": "string", "enum": ["rename", "overwrite", "skip"], "default": "rename" },
            "quality": { "type": "integer", "minimum": 1, "maximum": 100, "default": 90 },
            "long_edge": { "type": "integer", "minimum": 64 }
        }, "required": ["ids"] }),
        false,
    )
}

pub(super) async fn export_photos(app: &AppHandle, a: &Value) -> ToolResult {
    let state = app.state::<AppState>();
    let ids = ids(a, "ids")?;
    let name = |f: &str| match f {
        "png" => "png",
        "tiff" | "tif" => "tiff16",
        "dng" => "dng",
        _ => "jpeg",
    };
    let format = name(arg(a, "format").and_then(Value::as_str).unwrap_or("jpeg"));
    let formats: Vec<&str> = arg(a, "formats")
        .and_then(Value::as_array)
        .map(|l| l.iter().filter_map(Value::as_str).map(name).collect())
        .unwrap_or_default();
    let options: lumenkorn_develop::output::ExportOptions = serde_json::from_value(json!({
        "format": formats.first().copied().unwrap_or(format),
        "formats": formats,
        "dngEdited": !arg(a, "dng_original").and_then(Value::as_bool).unwrap_or(false),
        "onConflict": match arg(a, "on_conflict").and_then(Value::as_str) {
            Some("overwrite") => "overwrite",
            Some("skip") => "skip",
            _ => "rename",
        },
        "quality": arg(a, "quality").and_then(Value::as_u64).unwrap_or(90).clamp(1, 100),
        "longEdge": arg(a, "long_edge").and_then(Value::as_u64),
    }))
    .map_err(|x| x.to_string())?;
    if ids.len() > MAX_EXPORT {
        return Err(format!(
            "too many photos for one export (max {MAX_EXPORT}); split the list"
        ));
    }
    // Zielordner nur innerhalb der Bibliothek oder des Bilder-Ordners
    let dir = match arg(a, "folder").and_then(Value::as_str) {
        Some(f) => {
            let mut allowed: Vec<std::path::PathBuf> = state
                .catalog
                .locked()
                .roots()
                .map_err(e)?
                .into_iter()
                .map(|r| r.path.into())
                .collect();
            allowed.extend(dirs::picture_dir());
            Some(check_export_folder(f, &allowed)?)
        }
        None => None,
    };
    // KI-Agenten ersetzen nie ein Foto der Bibliothek (der Nutzer bestätigt hier nichts)
    crate::develop::start_export(
        app.clone(),
        ids.clone(),
        dir.clone(),
        options,
        crate::develop::Keep::Library,
    )
    .map_err(e)?;
    text(json!({
        "exporting": ids.len(),
        "to": dir.unwrap_or_else(|| "the export subfolder from the settings, next to each original".into()),
        "note": "Runs in the background; progress is shown in the app.",
    }))
}

pub(super) fn show_photo_tool() -> Value {
    tool(
        "show_photo",
        "Show a photo in the app",
        "Open a photo in Lumenkorn's window (library or develop view) so the user sees it.",
        json!({ "type": "object", "properties": {
            "id": { "type": "integer" },
            "develop": { "type": "boolean", "default": false }
        }, "required": ["id"] }),
        false,
    )
}

pub(super) async fn show_photo(app: &AppHandle, a: &Value) -> ToolResult {
    let state = app.state::<AppState>();
    let id = int(a, "id")?;
    state.catalog.locked().thumb_job(id).map_err(e)?;
    let develop = arg(a, "develop").and_then(Value::as_bool).unwrap_or(false);
    let _ = app.emit("mcp-show", json!({ "id": id, "develop": develop }));
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
    text(json!({ "shown": id }))
}

pub(super) fn flatten(
    n: &lumenkorn_core::model::FolderNode,
    depth: usize,
    out: &mut Vec<Value>,
    max: usize,
) {
    if out.len() >= max {
        return;
    }
    out.push(
        json!({ "path": n.path, "photos": n.count, "withSubfolders": n.total, "depth": depth }),
    );
    for c in &n.children {
        flatten(c, depth + 1, out, max);
    }
}

/// JSON Merge Patch (RFC 7396): Objekte zusammenführen, alles andere ersetzen
pub(super) fn merge(target: &mut Value, patch: &Value) {
    match (target, patch) {
        (Value::Object(t), Value::Object(p)) => {
            for (k, v) in p {
                if v.is_null() && k != "crop" && k != "denoise" {
                    t.remove(k);
                } else if let Some(slot) = t.get_mut(k)
                    && slot.is_object()
                    && v.is_object()
                {
                    merge(slot, v);
                } else {
                    t.insert(k.clone(), v.clone());
                }
            }
        }
        (t, p) => *t = p.clone(),
    }
}

/// Nur die Einstellungen, die vom Standard abweichen (Masken als Anzahl)
pub(super) fn non_default(p: &DevelopParams) -> Value {
    let cur = serde_json::to_value(p).unwrap_or_default();
    let def = serde_json::to_value(DevelopParams::default()).unwrap_or_default();
    let mut out = serde_json::Map::new();
    if let (Some(c), Some(d)) = (cur.as_object(), def.as_object()) {
        for (k, v) in c {
            if d.get(k) == Some(v) || k == "version" {
                continue;
            }
            let shown = match k.as_str() {
                "masks" | "repairs" => json!(v.as_array().map_or(0, Vec::len)),
                _ => v.clone(),
            };
            out.insert(k.clone(), shown);
        }
    }
    Value::Object(out)
}

pub(super) async fn search(app: &AppHandle, a: &Value) -> ToolResult {
    let state = app.state::<AppState>();
    let scope = if let Some(f) = arg(a, "folder").and_then(Value::as_str) {
        Scope::Folder {
            path: f.to_string(),
            recursive: arg(a, "include_subfolders")
                .and_then(Value::as_bool)
                .unwrap_or(true),
        }
    } else if let Some(id) = arg(a, "project_id").and_then(Value::as_i64) {
        Scope::Project { id }
    } else if let Some(id) = arg(a, "person_id").and_then(Value::as_i64) {
        Scope::Person { id }
    } else {
        Scope::All
    };
    let min_rating = arg(a, "min_rating")
        .and_then(Value::as_i64)
        .filter(|r| *r > 0);
    let exts = arg(a, "file_types")
        .and_then(Value::as_array)
        .map(|v| {
            let types: Vec<&str> = v.iter().filter_map(Value::as_str).collect();
            lumenkorn_core::formats::expand_file_types(&types)
        })
        .filter(|e| !e.is_empty());
    let text_q = arg(a, "text")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let by_content = arg(a, "by_content")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut list = if let (Some(q), true) = (text_q, by_content) {
        crate::ai::semantic_search(app.clone(), q.to_string(), scope, min_rating, exts, None)
            .await
            .map_err(e)?
    } else {
        let mut q = ImageQuery::new(scope);
        q.sort = match arg(a, "sort").and_then(Value::as_str) {
            Some("newest") => SortOrder::CapturedDesc,
            Some("name") => SortOrder::NameAsc,
            Some("rating") => SortOrder::RatingDesc,
            _ => SortOrder::CapturedAsc,
        };
        q.search = text_q.map(str::to_string);
        q.min_rating = min_rating;
        q.exts = exts;
        crate::commands::query_images(state.clone(), q).map_err(e)?
    };
    if let Some(f) = arg(a, "flag").and_then(Value::as_str) {
        let want = match f {
            "pick" => 1,
            "rejected" => -1,
            _ => 0,
        };
        list.retain(|i| i.flag == want);
    }
    if let Some(ed) = arg(a, "edited").and_then(Value::as_bool) {
        list.retain(|i| i.edited == ed);
    }
    let total = list.len();
    let offset = arg(a, "offset").and_then(Value::as_u64).unwrap_or(0) as usize;
    let limit = arg(a, "limit")
        .and_then(Value::as_u64)
        .unwrap_or(50)
        .clamp(1, 500) as usize;
    let page: Vec<Value> = list
        .iter()
        .skip(offset)
        .take(limit)
        .map(|i| {
            json!({
                "id": i.id, "file": i.file_name, "path": i.path, "date": i.captured_at,
                "rating": i.rating, "flag": i.flag, "edited": i.edited,
                "size": i.width.zip(i.height).map(|(w, h)| format!("{w}×{h}")),
            })
        })
        .collect();
    text(json!({ "total": total, "offset": offset, "photos": page }))
}

pub(super) async fn add_mask(app: &AppHandle, a: &Value) -> ToolResult {
    use lumenkorn_develop::params::{LocalAdjust, MaskLayer, MaskShape};
    let state = app.state::<AppState>();
    let id = int(a, "id")?;
    let kind = arg(a, "kind")
        .and_then(Value::as_str)
        .ok_or("`kind` is required")?;
    let (stored_kind, result) = match kind {
        "subject" => (
            "subject".to_string(),
            crate::ai::mask_subject(app.clone(), id).await,
        ),
        "sky" => (
            "sky".to_string(),
            crate::ai::mask_sky(app.clone(), id).await,
        ),
        k if k == "person" || k.starts_with("person:") => {
            let part = k.strip_prefix("person:").unwrap_or("person");
            let p: lumenkorn_ai::parts::PersonPart = serde_json::from_value(json!(part))
                .map_err(|_| format!("unknown person part: {part}"))?;
            (
                format!("person:{part}"),
                crate::ai::mask_person(app.clone(), id, p).await,
            )
        }
        k if k == "animal" || k.starts_with("animal:") => {
            let which = k.strip_prefix("animal:").unwrap_or("all");
            let p: lumenkorn_ai::parts::AnimalKind = serde_json::from_value(json!(which))
                .map_err(|_| format!("unknown animal kind: {which}"))?;
            (
                format!("animal:{which}"),
                crate::ai::mask_animal(app.clone(), id, p).await,
            )
        }
        other => return Err(format!("unknown mask kind: {other}")),
    };
    let r = result.map_err(e)?;
    if r.coverage <= 0.0 {
        return Err(format!(
            "Nothing found for `{kind}` in photo {id}; no mask added."
        ));
    }
    let adjust: LocalAdjust =
        serde_json::from_value(arg(a, "adjust").cloned().unwrap_or(json!({})))
            .map_err(|x| format!("adjust: {x}"))?;
    keep_edits(app, &[id])?;
    let mut p = crate::develop::develop_params(state.clone(), id).map_err(e)?;
    let name = arg(a, "name")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("KI: {kind}"));
    p.masks.push(MaskLayer {
        id: format!(
            "m{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis())
        ),
        name,
        enabled: true,
        shapes: vec![MaskShape::Raster {
            mask_id: r.mask_id,
            kind: stored_kind,
            prompt: vec![],
            invert: arg(a, "invert").and_then(Value::as_bool).unwrap_or(false),
            subtract: false,
        }],
        adjust,
    });
    crate::develop::develop_save(app.clone(), state.clone(), vec![id], p).map_err(e)?;
    changed(app, &[id]);
    text(json!({ "added": kind, "coverage": (r.coverage * 1000.0).round() / 1000.0 }))
}
