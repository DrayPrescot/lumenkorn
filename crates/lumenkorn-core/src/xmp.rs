//! XMP-Begleitdateien: vom Nutzer bearbeitete Metadaten (Titel, Autor,
//! Aufnahmedaten …) stehen zusätzlich zum Katalog in einer `.xmp`-Datei neben
//! dem Foto, damit andere Programme sie sehen. Die Bilddatei selbst bleibt
//! unberührt.
//!
//! Bestehende Begleitdateien (Lightroom, darktable, Capture One) enthalten
//! wichtige Fremddaten und werden nur ergänzt: Der Text wird als Folge von
//! Ereignissen gelesen, und nur die eigenen Eigenschaften werden am Byte-
//! Bereich ersetzt, entfernt oder eingefügt. Alles andere bleibt, wie es ist.
//! Lässt sich eine Datei nicht lesen, fasst Lumenkorn sie nicht an.

use crate::error::{Error, Result};
use lumenkorn_develop::tr;
use quick_xml::{Reader, events::Event};
use std::path::{Path, PathBuf};

pub const NS_RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";

/// Name einer XMP-Eigenschaft samt Namensraum
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PropName {
    pub prefix: &'static str,
    pub ns: &'static str,
    pub local: &'static str,
}

const fn prop(prefix: &'static str, ns: &'static str, local: &'static str) -> PropName {
    PropName { prefix, ns, local }
}

const NS_DC: &str = "http://purl.org/dc/elements/1.1/";
const NS_XMP: &str = "http://ns.adobe.com/xap/1.0/";
const NS_EXIF: &str = "http://ns.adobe.com/exif/1.0/";
const NS_TIFF: &str = "http://ns.adobe.com/tiff/1.0/";
const NS_AUX: &str = "http://ns.adobe.com/exif/1.0/aux/";
const NS_EXIF_EX: &str = "http://cipa.jp/exif/1.0/";
const NS_PHOTOSHOP: &str = "http://ns.adobe.com/photoshop/1.0/";

pub const DC_TITLE: PropName = prop("dc", NS_DC, "title");
pub const DC_DESCRIPTION: PropName = prop("dc", NS_DC, "description");
pub const DC_CREATOR: PropName = prop("dc", NS_DC, "creator");
pub const DC_RIGHTS: PropName = prop("dc", NS_DC, "rights");
pub const EXIF_DATE_ORIGINAL: PropName = prop("exif", NS_EXIF, "DateTimeOriginal");
pub const XMP_CREATE_DATE: PropName = prop("xmp", NS_XMP, "CreateDate");
pub const PS_DATE_CREATED: PropName = prop("photoshop", NS_PHOTOSHOP, "DateCreated");
pub const TIFF_MODEL: PropName = prop("tiff", NS_TIFF, "Model");
pub const EXIF_EX_LENS: PropName = prop("exifEX", NS_EXIF_EX, "LensModel");
pub const AUX_LENS: PropName = prop("aux", NS_AUX, "Lens");
pub const EXIF_FOCAL_LENGTH: PropName = prop("exif", NS_EXIF, "FocalLength");
pub const EXIF_F_NUMBER: PropName = prop("exif", NS_EXIF, "FNumber");
pub const EXIF_EXPOSURE_TIME: PropName = prop("exif", NS_EXIF, "ExposureTime");
pub const EXIF_ISO: PropName = prop("exif", NS_EXIF, "ISOSpeedRatings");
pub const DC_SUBJECT: PropName = prop("dc", NS_DC, "subject");
pub const XMP_RATING: PropName = prop("xmp", NS_XMP, "Rating");

/// Inhalt einer Eigenschaft
#[derive(Debug, Clone, PartialEq)]
pub enum Content {
    /// Einfacher Wert
    Plain(String),
    /// Sprachvariante (`rdf:Alt`, nur `x-default`)
    Alt(String),
    /// Geordnete Liste mit einem Eintrag (`rdf:Seq`)
    Seq(String),
    /// Ungeordnete Liste (`rdf:Bag`), etwa Schlagworte
    Bag(Vec<String>),
}

/// Änderung einer Eigenschaft: `value = None` entfernt sie.
#[derive(Debug, Clone, PartialEq)]
pub struct XmpChange {
    pub name: PropName,
    pub value: Option<Content>,
}

/// Neue Begleitdatei: ein gültiges Standardpaket mit leerer Beschreibung
const TEMPLATE: &str = "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"Lumenkorn\">\n\
 <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
  <rdf:Description rdf:about=\"\">\n\
  </rdf:Description>\n\
 </rdf:RDF>\n\
</x:xmpmeta>\n\
<?xpacket end=\"w\"?>\n";

fn xmp_err(detail: impl std::fmt::Display) -> Error {
    Error::Xmp(detail.to_string())
}

// ----------------------------------------------------------------------
// Begleitdatei suchen
// ----------------------------------------------------------------------

/// XMP-Begleitdateien eines Fotos: `foto.cr3.xmp` gehört immer dazu,
/// `foto.xmp` nur, wenn kein anderes Foto gleichen Namens daneben liegt
/// (RAW+JPEG-Paare teilen sich diese Datei).
pub fn sidecars(photo: &Path) -> Vec<PathBuf> {
    crate::rename::sidecars_of(&[photo])
}

/// Begleitdatei, in die Änderungen gehören: eine vorhandene (`foto.jpg.xmp`
/// vor `foto.xmp`), sonst die zu erstellende – `foto.xmp` (Lightroom, Bridge,
/// Capture One), bei einem Paar gleichen Namens `foto.jpg.xmp`.
pub fn sidecar_for_edit(photo: &Path) -> PathBuf {
    let existing = sidecars(photo);
    let name = photo
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let full = format!("{name}.xmp").to_ascii_lowercase();
    let pick = existing
        .iter()
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.to_ascii_lowercase() == full)
        })
        .or(existing.first());
    if let Some(p) = pick {
        return p.clone();
    }
    // Keine vorhanden: `foto.xmp`, außer ein Foto gleichen Namens liegt daneben
    let dir = photo.parent().unwrap_or(Path::new("."));
    let stem = photo.file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    let sibling = std::fs::read_dir(dir).is_ok_and(|rd| {
        rd.filter_map(|e| e.ok()?.file_name().into_string().ok())
            .any(|n| {
                !n.to_ascii_lowercase().ends_with(".xmp")
                    && n != name
                    && Path::new(&n)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .is_some_and(|s| s == stem)
            })
    });
    dir.join(if sibling {
        format!("{name}.xmp")
    } else {
        format!("{stem}.xmp")
    })
}

/// Änderungen in die Begleitdatei des Fotos schreiben (siehe
/// [`sidecar_for_edit`]). Eine bestehende Datei wird ergänzt, nie ersetzt;
/// lässt sie sich nicht lesen, bleibt sie unverändert und es gibt einen
/// Fehler. Ohne Datei und ohne zu setzende Werte (nur Entfernen) entsteht
/// keine. Schreibt über eine Zwischendatei im selben Ordner.
pub fn update_sidecar(photo: &Path, changes: &[XmpChange]) -> Result<Option<PathBuf>> {
    if changes.is_empty() {
        return Ok(None);
    }
    let path = sidecar_for_edit(photo);
    let existing = match std::fs::read(&path) {
        Ok(bytes) => Some(String::from_utf8(bytes).map_err(|_| {
            xmp_err(tr!(
                "{} ist nicht UTF-8-kodiert und bleibt unverändert",
                "{} is not UTF-8 encoded and was left unchanged",
                path.display()
            ))
        })?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    if existing.is_none() && changes.iter().all(|c| c.value.is_none()) {
        return Ok(None);
    }
    let merged = merge(existing.as_deref(), changes).map_err(|e| match e {
        Error::Xmp(d) => xmp_err(tr!(
            "{} kann nicht gelesen werden und bleibt unverändert ({d})",
            "{} cannot be read and was left unchanged ({d})",
            path.display()
        )),
        e => e,
    })?;
    if existing.as_deref() == Some(merged.as_str()) {
        return Ok(Some(path));
    }
    write_atomic(&path, merged.as_bytes())?;
    Ok(Some(path))
}

fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    Ok(lumenkorn_develop::fsutil::write_atomic(path, data)?)
}

// ----------------------------------------------------------------------
// Zusammenführen
// ----------------------------------------------------------------------

/// Ein gelesenes Element, an dem Text eingefügt werden kann
#[derive(Debug, Clone, Default)]
struct Elem {
    qname: String,
    tag_start: usize,
    /// Ende des Anfangs-Tags
    tag_end: usize,
    /// Stelle im Anfangs-Tag, an der Attribute enden (vor `>` bzw. `/>`)
    attrs_end: usize,
    empty: bool,
    /// Beginn des Ende-Tags
    close_start: usize,
    /// Namensräume in Reichweite (Präfix, Adresse)
    scope: Vec<(String, String)>,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Other,
    Rdf,
    Desc,
}

struct Attr {
    name: String,
    /// Bereich samt vorangehendem Leerraum
    span: (usize, usize),
    value: String,
}

/// Zerlegt ein Anfangs-Tag (`<name a="b" …>`). `base`: Lage von `<` im Text.
/// Liefert Attribute und die Stelle, an der sie enden.
fn parse_tag(tag: &str, base: usize) -> Result<(Vec<Attr>, usize)> {
    let b = tag.as_bytes();
    let bad = || xmp_err("tag");
    let mut i = 1;
    while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'/' && b[i] != b'>' {
        i += 1;
    }
    let mut attrs = Vec::new();
    loop {
        let ws_start = i;
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() || b[i] == b'/' || b[i] == b'>' {
            return Ok((attrs, base + ws_start));
        }
        let n0 = i;
        while i < b.len() && b[i] != b'=' && !b[i].is_ascii_whitespace() {
            i += 1;
        }
        let name = tag[n0..i].to_string();
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if b.get(i) != Some(&b'=') {
            return Err(bad());
        }
        i += 1;
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let q = *b.get(i).ok_or_else(bad)?;
        if q != b'"' && q != b'\'' {
            return Err(bad());
        }
        let v0 = i + 1;
        let v1 = v0 + tag[v0..].find(q as char).ok_or_else(bad)?;
        i = v1 + 1;
        attrs.push(Attr {
            name,
            span: (base + ws_start, base + i),
            value: tag[v0..v1].to_string(),
        });
    }
}

fn split_qname(q: &str) -> (&str, &str) {
    q.split_once(':').unwrap_or(("", q))
}

fn lookup<'a>(scope: &'a [(String, String)], prefix: &str) -> Option<&'a str> {
    scope
        .iter()
        .rev()
        .find(|(p, _)| p == prefix)
        .map(|(_, u)| u.as_str())
}

fn extend_back(src: &str, mut pos: usize) -> usize {
    let b = src.as_bytes();
    while pos > 0 && b[pos - 1].is_ascii_whitespace() {
        pos -= 1;
    }
    pos
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            // Steuerzeichen sind in XML 1.0 nicht erlaubt
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
            c => out.push(c),
        }
    }
    out
}

struct Scan {
    rdf: Option<Elem>,
    descs: Vec<Elem>,
    /// Bereiche, die entfernt werden (eigene Eigenschaften in beiden Formen)
    removals: Vec<(usize, usize)>,
}

fn scan(src: &str, names: &[PropName]) -> Result<Scan> {
    let ours = |ns: &str, local: &str| names.iter().any(|n| n.ns == ns && n.local == local);
    let mut reader = Reader::from_str(src);
    // Kind, eigene Namensräume, Index in `descs`
    #[allow(clippy::type_complexity)]
    let mut stack: Vec<(Kind, Vec<(String, String)>, usize)> = Vec::new();
    let mut out = Scan {
        rdf: None,
        descs: Vec::new(),
        removals: Vec::new(),
    };
    // Eigene Eigenschaft als Kindelement offen: (Tiefe, Beginn)
    let mut open_prop: Option<(usize, usize)> = None;
    let mut seen_root = false;
    loop {
        let before = reader.buffer_position() as usize;
        let ev = reader.read_event().map_err(xmp_err)?;
        let after = reader.buffer_position() as usize;
        let empty = matches!(ev, Event::Empty(_));
        match ev {
            Event::Start(_) | Event::Empty(_) => {
                let tag = &src[before..after];
                let (attrs, attrs_end) = parse_tag(tag, before)?;
                let qname_end = tag[1..]
                    .find(|c: char| c.is_ascii_whitespace() || c == '/' || c == '>')
                    .map_or(tag.len(), |p| p + 1);
                let qname = tag[1..qname_end].to_string();
                let mut scope: Vec<(String, String)> =
                    stack.iter().flat_map(|(_, s, _)| s.clone()).collect();
                let mut own = Vec::new();
                for a in &attrs {
                    if a.name == "xmlns" {
                        own.push((String::new(), a.value.clone()));
                    } else if let Some(p) = a.name.strip_prefix("xmlns:") {
                        own.push((p.to_string(), a.value.clone()));
                    }
                }
                scope.extend(own.iter().cloned());
                let (prefix, local) = split_qname(&qname);
                let ns = lookup(&scope, prefix).unwrap_or("");
                let parent = stack.last().map(|s| s.0);
                let kind = if ns == NS_RDF && local == "RDF" {
                    Kind::Rdf
                } else if ns == NS_RDF && local == "Description" && parent == Some(Kind::Rdf) {
                    Kind::Desc
                } else {
                    Kind::Other
                };
                if stack.is_empty() {
                    seen_root = true;
                }
                let elem = Elem {
                    qname: qname.clone(),
                    tag_start: before,
                    tag_end: after,
                    attrs_end,
                    empty,
                    close_start: after,
                    scope: scope.clone(),
                };
                if parent == Some(Kind::Desc) && ours(ns, local) {
                    if empty {
                        out.removals.push((extend_back(src, before), after));
                    } else if open_prop.is_none() {
                        open_prop = Some((stack.len() + 1, extend_back(src, before)));
                    }
                }
                if kind == Kind::Desc {
                    for a in &attrs {
                        let (p, l) = split_qname(&a.name);
                        if !p.is_empty()
                            && p != "xmlns"
                            && lookup(&scope, p).is_some_and(|u| ours(u, l))
                        {
                            out.removals.push(a.span);
                        }
                    }
                    out.descs.push(elem.clone());
                } else if kind == Kind::Rdf && out.rdf.is_none() {
                    out.rdf = Some(elem.clone());
                }
                if !empty {
                    let idx = if kind == Kind::Desc {
                        out.descs.len() - 1
                    } else {
                        0
                    };
                    stack.push((kind, own, idx));
                }
            }
            Event::End(_) => {
                let (kind, _, idx) = stack.pop().ok_or_else(|| xmp_err("end"))?;
                match kind {
                    Kind::Desc => out.descs[idx].close_start = before,
                    Kind::Rdf => {
                        // Nur das erste rdf:RDF zählt (gleicher Beginn)
                        if let Some(r) = out.rdf.as_mut()
                            && r.close_start == r.tag_end
                        {
                            r.close_start = before;
                        }
                    }
                    Kind::Other => {}
                }
                if let Some((depth, start)) = open_prop
                    && stack.len() + 1 == depth
                {
                    out.removals.push((start, after));
                    open_prop = None;
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !stack.is_empty() || !seen_root {
        return Err(xmp_err("structure"));
    }
    Ok(out)
}

/// Eigenschaft als XML-Text
fn render(name: &PropName, prefix: &str, rdf: &str, c: &Content) -> String {
    let n = format!("{prefix}:{}", name.local);
    match c {
        Content::Plain(v) => format!("<{n}>{}</{n}>", escape(v)),
        Content::Alt(v) => format!(
            "<{n}><{rdf}:Alt><{rdf}:li xml:lang=\"x-default\">{}</{rdf}:li></{rdf}:Alt></{n}>",
            escape(v)
        ),
        Content::Seq(v) => format!(
            "<{n}><{rdf}:Seq><{rdf}:li>{}</{rdf}:li></{rdf}:Seq></{n}>",
            escape(v)
        ),
        Content::Bag(v) => format!(
            "<{n}><{rdf}:Bag>{}</{rdf}:Bag></{n}>",
            v.iter()
                .map(|s| format!("<{rdf}:li>{}</{rdf}:li>", escape(s)))
                .collect::<String>()
        ),
    }
}

/// Fügt `items` als Kinder an `parent` an (vor dem Ende-Tag, mit dem
/// Einzug der vorhandenen Zeilen). `decls`: zusätzliche Namensräume im Tag.
fn append_children(
    edits: &mut Vec<(usize, usize, String)>,
    src: &str,
    parent: &Elem,
    decls: &str,
    items: &[String],
) {
    if parent.empty {
        // `<x a="b"/>` → `<x a="b" xmlns:…>…</x>`
        let line_start = src[..parent.tag_start].rfind('\n').map_or(0, |p| p + 1);
        let ind: String = src[line_start..parent.tag_start]
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect();
        let mut s = format!("{decls}>");
        for it in items {
            s.push_str(&format!("\n{ind}  {it}"));
        }
        s.push_str(&format!("\n{ind}</{}>", parent.qname));
        edits.push((parent.attrs_end, parent.tag_end, s));
        return;
    }
    if !decls.is_empty() {
        edits.push((parent.attrs_end, parent.attrs_end, decls.to_string()));
    }
    if items.is_empty() {
        return;
    }
    let ws_start = extend_back(src, parent.close_start);
    let trailing = &src[ws_start..parent.close_start];
    if let Some(nl) = trailing.rfind('\n') {
        let ind = &trailing[nl + 1..];
        let s: String = items.iter().map(|it| format!("\n{ind}  {it}")).collect();
        edits.push((ws_start, ws_start, s));
    } else {
        edits.push((parent.close_start, parent.close_start, items.concat()));
    }
}

/// Führt `changes` in ein XMP-Paket ein (`None`: neues Paket). Eigene
/// Eigenschaften werden überall entfernt (als Attribut oder Element) und, wenn
/// sie einen Wert haben, in der ersten `rdf:Description` neu geschrieben.
/// Alles andere bleibt Byte für Byte erhalten.
pub fn merge(existing: Option<&str>, changes: &[XmpChange]) -> Result<String> {
    let src = existing.unwrap_or(TEMPLATE);
    let names: Vec<PropName> = changes.iter().map(|c| c.name).collect();
    let sc = scan(src, &names)?;
    let rdf = sc.rdf.as_ref().ok_or_else(|| xmp_err("rdf:RDF"))?;
    let target = sc.descs.first();
    let scope = target.map_or(&rdf.scope, |d| &d.scope);

    let mut edits: Vec<(usize, usize, String)> = sc
        .removals
        .iter()
        .map(|&(a, b)| (a, b, String::new()))
        .collect();

    // Präfixe: vorhandene Zuordnung nutzen, sonst deklarieren
    let mut decls: Vec<(String, &str)> = Vec::new();
    let mut prefix_for = |preferred: &str, uri: &'static str| -> String {
        if let Some((p, _)) = scope
            .iter()
            .rev()
            .find(|(p, u)| u == uri && !p.is_empty() && lookup(scope, p) == Some(uri))
        {
            return p.clone();
        }
        if let Some((p, _)) = decls.iter().find(|(_, u)| *u == uri) {
            return p.clone();
        }
        let mut p = preferred.to_string();
        let mut n = 2;
        while lookup(scope, &p).is_some() || decls.iter().any(|(q, _)| *q == p) {
            p = format!("{preferred}{n}");
            n += 1;
        }
        decls.push((p.clone(), uri));
        p
    };
    let mut items = Vec::new();
    let need_rdf = changes.iter().any(|c| {
        matches!(
            c.value,
            Some(Content::Alt(_) | Content::Seq(_) | Content::Bag(_))
        )
    });
    let rdf_prefix = if need_rdf || target.is_none() {
        prefix_for("rdf", NS_RDF)
    } else {
        String::new()
    };
    for c in changes {
        if let Some(v) = &c.value {
            let p = prefix_for(c.name.prefix, c.name.ns);
            items.push(render(&c.name, &p, &rdf_prefix, v));
        }
    }
    let decl_text: String = decls
        .iter()
        .map(|(p, u)| format!(" xmlns:{p}=\"{u}\""))
        .collect();

    match target {
        Some(d) => append_children(&mut edits, src, d, &decl_text, &items),
        None => {
            // rdf:RDF ohne Beschreibung: neue anlegen
            let rdf_p = lookup_prefix(&rdf.scope, NS_RDF)
                .ok_or_else(|| xmp_err("rdf prefix"))?
                .to_string();
            let desc = if items.is_empty() {
                String::new()
            } else {
                let inner = items.join("");
                format!(
                    "<{rdf_p}:Description {rdf_p}:about=\"\"{decl_text}>{inner}</{rdf_p}:Description>"
                )
            };
            if !desc.is_empty() {
                append_children(&mut edits, src, rdf, "", &[desc]);
            }
        }
    }

    edits.sort_by_key(|e| std::cmp::Reverse((e.0, e.1)));
    let mut out = src.to_string();
    let mut last_start = usize::MAX;
    for (a, b, text) in edits {
        if b > last_start || a > b {
            return Err(xmp_err("overlap"));
        }
        out.replace_range(a..b, &text);
        last_start = a;
    }
    // Ergebnis muss wieder lesbar sein, sonst nichts schreiben
    scan(&out, &names)?;
    Ok(out)
}

fn lookup_prefix<'a>(scope: &'a [(String, String)], uri: &str) -> Option<&'a str> {
    scope
        .iter()
        .rev()
        .find(|(p, u)| u == uri && !p.is_empty())
        .map(|(p, _)| p.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn title(v: &str) -> XmpChange {
        XmpChange {
            name: DC_TITLE,
            value: Some(Content::Alt(v.into())),
        }
    }

    fn remove(name: PropName) -> XmpChange {
        XmpChange { name, value: None }
    }

    const LIGHTROOM: &str = r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core 7.0">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:exif="http://ns.adobe.com/exif/1.0/"
   crs:Exposure2012="+0.35"
   crs:Contrast2012="+10"
   xmp:Rating="3"
   xmp:CreateDate="2020-01-01T00:00:00"
   exif:ISOSpeedRatings="100">
   <crs:ToneCurvePV2012>
    <rdf:Seq>
     <rdf:li>0, 0</rdf:li>
     <rdf:li>255, 255</rdf:li>
    </rdf:Seq>
   </crs:ToneCurvePV2012>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#;

    const DARKTABLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="XMP Core 4.4.0-Exiv2">
  <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
    <rdf:Description rdf:about=""
      xmlns:xmpMM="http://ns.adobe.com/xap/1.0/mm/"
      xmlns:darktable="http://darktable.sf.net/"
      xmlns:dc="http://purl.org/dc/elements/1.1/"
      darktable:xmp_version="5"
      darktable:raw_params="0">
      <dc:title>
        <rdf:Alt>
          <rdf:li xml:lang="x-default">Alt</rdf:li>
        </rdf:Alt>
      </dc:title>
      <darktable:history>
        <rdf:Seq>
          <rdf:li darktable:num="0" darktable:operation="exposure"/>
        </rdf:Seq>
      </darktable:history>
    </rdf:Description>
  </rdf:RDF>
</x:xmpmeta>
"#;

    #[test]
    fn creates_valid_packet() {
        let out = merge(
            None,
            &[
                title("Ein <Titel> & \"mehr\""),
                XmpChange {
                    name: DC_CREATOR,
                    value: Some(Content::Seq("Jörg Müller 日本語".into())),
                },
                XmpChange {
                    name: EXIF_F_NUMBER,
                    value: Some(Content::Plain("28/10".into())),
                },
            ],
        )
        .unwrap();
        assert!(out.starts_with("<?xpacket begin"));
        assert!(out.contains("xmlns:dc=\"http://purl.org/dc/elements/1.1/\""));
        assert!(out.contains("<dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">Ein &lt;Titel&gt; &amp; &quot;mehr&quot;</rdf:li>"));
        assert!(out.contains("<rdf:li>Jörg Müller 日本語</rdf:li>"));
        assert!(out.contains("<exif:FNumber>28/10</exif:FNumber>"));
        assert!(out.contains("rdf:about=\"\""));
        // wieder lesbar, nochmal ändern ersetzt statt zu verdoppeln
        let again = merge(Some(&out), &[title("Neu")]).unwrap();
        assert_eq!(again.matches("<dc:title>").count(), 1);
        assert!(again.contains(">Neu<"));
        assert!(!again.contains("mehr"));
    }

    #[test]
    fn merges_into_lightroom_sample_keeping_foreign_data() {
        let out = merge(
            Some(LIGHTROOM),
            &[
                title("Sonnenuntergang"),
                XmpChange {
                    name: EXIF_ISO,
                    value: Some(Content::Seq("400".into())),
                },
                XmpChange {
                    name: XMP_CREATE_DATE,
                    value: Some(Content::Plain("2024-06-01T14:03:22".into())),
                },
            ],
        )
        .unwrap();
        // Fremddaten unverändert
        for keep in [
            "crs:Exposure2012=\"+0.35\"",
            "xmp:Rating=\"3\"",
            "<rdf:li>0, 0</rdf:li>",
            "<?xpacket end=\"w\"?>",
            "x:xmptk=\"Adobe XMP Core 7.0\"",
        ] {
            assert!(out.contains(keep), "fehlt: {keep}");
        }
        // Attributform entfernt, Elementform geschrieben
        assert!(!out.contains("exif:ISOSpeedRatings=\"100\""));
        assert!(!out.contains("xmp:CreateDate=\"2020"));
        assert!(out.contains("<exif:ISOSpeedRatings><rdf:Seq><rdf:li>400</rdf:li>"));
        assert!(out.contains("<xmp:CreateDate>2024-06-01T14:03:22</xmp:CreateDate>"));
        // dc war nicht deklariert
        assert!(out.contains("xmlns:dc=\"http://purl.org/dc/elements/1.1/\""));
        // vorhandene Präfixe wiederverwendet, nicht doppelt deklariert
        assert_eq!(out.matches("xmlns:xmp=").count(), 1);
        assert_eq!(out.matches("xmlns:exif=").count(), 1);
    }

    #[test]
    fn merges_into_darktable_sample() {
        let out = merge(Some(DARKTABLE), &[title("Neu"), remove(EXIF_ISO)]).unwrap();
        assert!(out.contains("darktable:operation=\"exposure\""));
        assert!(out.contains("darktable:xmp_version=\"5\""));
        assert!(!out.contains(">Alt<"));
        assert!(out.contains(">Neu<"));
        assert_eq!(out.matches("<dc:title>").count(), 1);
        // Einzug der Datei bleibt gewahrt
        assert!(out.ends_with("</x:xmpmeta>\n"));
    }

    #[test]
    fn untouched_bytes_stay_identical() {
        let out = merge(Some(DARKTABLE), &[title("Alt2")]).unwrap();
        let (a, b) = (
            DARKTABLE.find("<darktable:history>").unwrap(),
            DARKTABLE.find("</darktable:history>").unwrap(),
        );
        assert!(out.contains(&DARKTABLE[a..b]));
    }

    #[test]
    fn clearing_removes_both_forms() {
        let out = merge(
            Some(LIGHTROOM),
            &[remove(EXIF_ISO), remove(XMP_CREATE_DATE)],
        )
        .unwrap();
        assert!(!out.contains("ISOSpeedRatings"));
        assert!(!out.contains("CreateDate"));
        assert!(out.contains("xmp:Rating=\"3\""));
        let with = merge(None, &[title("T")]).unwrap();
        let without = merge(Some(&with), &[remove(DC_TITLE)]).unwrap();
        assert!(!without.contains("dc:title"));
        assert!(without.contains("<rdf:Description"));
    }

    #[test]
    fn empty_description_and_missing_description() {
        let empty = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description rdf:about=\"\" /></rdf:RDF></x:xmpmeta>";
        let out = merge(Some(empty), &[title("T")]).unwrap();
        assert!(out.contains("</rdf:Description>"));
        assert!(out.contains(">T<"));
        scan(&out, &[]).unwrap();

        let none = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"/></x:xmpmeta>";
        let out = merge(Some(none), &[title("T")]).unwrap();
        assert!(out.contains("<rdf:Description rdf:about=\"\""));
        scan(&out, &[]).unwrap();
    }

    #[test]
    fn foreign_prefix_for_dc_is_reused() {
        let src = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><r:RDF xmlns:r=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><r:Description r:about=\"\" xmlns:d=\"http://purl.org/dc/elements/1.1/\" d:title=\"x\"></r:Description></r:RDF></x:xmpmeta>";
        let out = merge(Some(src), &[title("Y")]).unwrap();
        assert!(!out.contains("d:title=\"x\""));
        assert!(
            out.contains("<d:title><r:Alt><r:li xml:lang=\"x-default\">Y</r:li></r:Alt></d:title>")
        );
    }

    #[test]
    fn unparsable_is_rejected() {
        assert!(merge(Some("<x:xmpmeta><rdf:RDF></x:xmpmeta>"), &[title("T")]).is_err());
        assert!(merge(Some("kein xml"), &[title("T")]).is_err());
        assert!(merge(Some("<a><b/></a>"), &[title("T")]).is_err());
    }

    #[test]
    fn sidecar_selection_and_files() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let touch = |n: &str| std::fs::write(d.join(n), b"x").unwrap();
        // allein: foto.xmp
        touch("a.jpg");
        assert_eq!(sidecar_for_edit(&d.join("a.jpg")), d.join("a.xmp"));
        // Paar: foto.jpg.xmp
        touch("b.jpg");
        touch("b.nef");
        assert_eq!(sidecar_for_edit(&d.join("b.jpg")), d.join("b.jpg.xmp"));
        // b.xmp gehört dem Paar → nicht für b.jpg
        touch("b.xmp");
        assert_eq!(sidecar_for_edit(&d.join("b.jpg")), d.join("b.jpg.xmp"));
        // darktable-Stil hat Vorrang
        touch("a.jpg.xmp");
        assert_eq!(sidecar_for_edit(&d.join("a.jpg")), d.join("a.jpg.xmp"));

        // Schreiben: neu, dann Ergänzen, Löschen ohne Datei legt nichts an
        let p = d.join("c.jpg");
        touch("c.jpg");
        assert_eq!(update_sidecar(&p, &[remove(DC_TITLE)]).unwrap(), None);
        assert!(!d.join("c.xmp").exists());
        let side = update_sidecar(&p, &[title("T")]).unwrap().unwrap();
        assert_eq!(side, d.join("c.xmp"));
        let text = std::fs::read_to_string(&side).unwrap();
        assert!(text.contains(">T<"));
        update_sidecar(&p, &[remove(DC_TITLE)]).unwrap();
        assert!(!std::fs::read_to_string(&side).unwrap().contains("dc:title"));
        // keine Zwischendatei übrig
        assert!(
            std::fs::read_dir(d).unwrap().all(|e| !e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("tmp"))
        );
    }

    #[test]
    fn unparsable_file_is_left_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let photo = dir.path().join("d.jpg");
        std::fs::write(&photo, b"x").unwrap();
        let side = dir.path().join("d.xmp");
        std::fs::write(&side, "<broken><rdf:RDF>").unwrap();
        let err = update_sidecar(&photo, &[title("T")]).unwrap_err();
        assert!(matches!(err, Error::Xmp(_)));
        assert_eq!(std::fs::read_to_string(&side).unwrap(), "<broken><rdf:RDF>");
    }
}
