//! Der Parametersatz einer Bearbeitung.
//!
//! Wird als JSON im Katalog gespeichert. Felder dürfen hinzukommen (mit
//! sinnvollem Standardwert via `#[serde(default)]`), aber nie ihre Bedeutung
//! ändern – sonst sähen alte Bearbeitungen plötzlich anders aus. Für echte
//! Brüche gibt es `version`; bisher gab es keinen, daher liest es noch niemand
//! (eine Migration käme beim Laden dazu).

use serde::{Deserialize, Serialize};

pub const CURRENT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DevelopParams {
    pub version: u32,

    // Weißabgleich, relativ zur Aufnahme. −100…100
    pub temperature: f32,
    pub tint: f32,

    // Licht
    /// Belichtung in Blendenstufen, −5…5
    pub exposure: f32,
    /// −100…100
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,

    // Präsenz
    /// Lokaler Kontrast in den Mitteltönen, −100…100
    pub clarity: f32,
    /// Sättigung, die bereits kräftige Farben und Hauttöne schont, −100…100
    pub vibrance: f32,
    pub saturation: f32,

    // Details
    /// Unscharf maskieren, 0…150
    pub sharpen: f32,

    /// Farbton, Sättigung und Luminanz je Farbbereich (HSL)
    pub hsl: Hsl,

    // Geometrie
    /// Vierteldrehungen im Uhrzeigersinn, 0…3
    pub rotate: u8,
    pub flip_horizontal: bool,
    /// Geraderichten in Grad, im Uhrzeigersinn positiv, −45…45. Das Bild wird so
    /// weit vergrößert, dass keine leeren Ecken entstehen.
    pub straighten: f32,
    /// Zuschnitt in normierten Koordinaten des gedrehten Bildes
    pub crop: Option<Crop>,

    /// Lokale Anpassungen, in Reihenfolge angewendet.
    pub masks: Vec<MaskLayer>,

    /// KI-Rauschreduzierung: Verweis auf das zwischengespeicherte Ergebnis.
    pub denoise: Option<DenoiseRef>,

    /// KI-Reparaturen (Bereiche aus der Umgebung neu füllen), in Reihenfolge.
    pub repairs: Vec<RepairLayer>,
}

/// Anzahl der HSL-Farbbereiche: Rot, Orange, Gelb, Grün, Aquamarin, Blau,
/// Lila, Magenta.
pub const HSL_BANDS: usize = 8;

/// HSL-Werkzeug: je Farbbereich Farbton, Sättigung und Luminanz, −100…100.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Hsl {
    pub hue: [f32; HSL_BANDS],
    pub saturation: [f32; HSL_BANDS],
    pub luminance: [f32; HSL_BANDS],
}

impl Hsl {
    pub fn is_neutral(&self) -> bool {
        *self == Hsl::default()
    }
}

/// Welche Teile einer Bearbeitung beim Übertragen mitgehen.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TransferGroups {
    /// Weißabgleich, Licht, Farbe (inkl. HSL), Klarheit, Schärfen
    pub basic: bool,
    /// Masken ohne KI-Masken
    pub masks: bool,
    /// Zuschnitt, Drehen, Spiegeln, Geraderichten
    pub geometry: bool,
}

impl Default for TransferGroups {
    fn default() -> Self {
        Self {
            basic: true,
            masks: false,
            geometry: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DenoiseRef {
    /// Datei-ID im Entrausch-Speicher
    pub id: String,
    /// Mischung mit dem Original, 0…100
    pub amount: f32,
}

/// Eine lokale Anpassung: Maske (aus einer oder mehreren Formen) plus Regler.
///
/// Alle Koordinaten sind normiert auf das *aufrechte Originalbild* (nach
/// EXIF-Orientierung, vor Drehen/Spiegeln/Zuschnitt). Ein späterer Zuschnitt
/// verschiebt eine Maske deshalb nie vom Motiv weg.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MaskLayer {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub shapes: Vec<MaskShape>,
    pub adjust: LocalAdjust,
}

impl Default for MaskLayer {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            enabled: true,
            shapes: vec![],
            adjust: LocalAdjust::default(),
        }
    }
}

/// Eine Reparatur: Auswahl (dieselben Formen wie bei Masken) und – nach dem
/// Berechnen – der Verweis auf den gespeicherten Flicken.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct RepairLayer {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub shapes: Vec<MaskShape>,
    /// ID des berechneten Flickens; `None` = noch nicht berechnet
    pub result: Option<String>,
    /// Fingerabdruck der Auswahl beim Berechnen (Oberfläche erkennt Änderungen)
    pub result_key: Option<String>,
}

impl RepairLayer {
    /// Als Maskenebene (für Auswertung und rote Überlagerung)
    pub fn as_mask(&self) -> MaskLayer {
        MaskLayer {
            id: self.id.clone(),
            name: self.name.clone(),
            enabled: false,
            shapes: self.shapes.clone(),
            adjust: LocalAdjust::default(),
        }
    }
}

/// Bestandteil einer Maske. `subtract` zieht die Form von der bisherigen Maske ab.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum MaskShape {
    /// Verlauf: voll wirksam bei (x0, y0), ausgeblendet bei (x1, y1).
    Linear {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        #[serde(default)]
        invert: bool,
        #[serde(default)]
        subtract: bool,
    },
    /// Ellipse; Radien relativ zur längeren Bildkante, Winkel in Grad.
    Radial {
        cx: f32,
        cy: f32,
        rx: f32,
        ry: f32,
        #[serde(default)]
        angle: f32,
        /// Anteil des Radius, über den weich ausgeblendet wird, 0…1
        #[serde(default = "default_feather")]
        feather: f32,
        #[serde(default)]
        invert: bool,
        #[serde(default)]
        subtract: bool,
    },
    /// Pinselstriche.
    Brush {
        strokes: Vec<Stroke>,
        #[serde(default)]
        subtract: bool,
    },
    /// Farbbereich: alle Pixel, deren Farbe einer der mit der Pipette
    /// gewählten ähnelt. Die Farben (Oklab) werden beim Klicken gemessen und
    /// gespeichert – so wirkt die Maske übertragen auf ein anderes Foto auf
    /// dieselbe Farbe, nicht dieselbe Stelle.
    Color {
        samples: Vec<ColorSample>,
        /// Toleranz 0…100
        #[serde(default = "default_color_range")]
        range: f32,
        #[serde(default)]
        invert: bool,
        #[serde(default)]
        subtract: bool,
    },
    /// Vom Modell berechnete Maske (Motiv, Himmel, Objekt). Das Pixelergebnis
    /// liegt als Datei vor; `prompt` hält fest, wie es entstanden ist.
    Raster {
        mask_id: String,
        kind: String,
        #[serde(default)]
        prompt: Vec<PromptPoint>,
        #[serde(default)]
        invert: bool,
        #[serde(default)]
        subtract: bool,
    },
}

fn default_feather() -> f32 {
    0.5
}

fn default_color_range() -> f32 {
    30.0
}

/// Ein Pipettenklick: Stelle (normiert, Originalausrichtung) und gemessene
/// Farbe als Oklab (L, a, b).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ColorSample {
    pub x: f32,
    pub y: f32,
    pub lab: [f32; 3],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Stroke {
    /// Punkte [x, y], normiert
    pub points: Vec<[f32; 2]>,
    /// Radius relativ zur längeren Bildkante
    pub radius: f32,
    /// Weiche Kante, 0…1
    pub feather: f32,
    /// Deckkraft, 0…1
    pub opacity: f32,
    /// Radiert statt malt
    pub erase: bool,
}

impl Default for Stroke {
    fn default() -> Self {
        Self {
            points: vec![],
            radius: 0.03,
            feather: 0.6,
            opacity: 1.0,
            erase: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PromptPoint {
    pub x: f32,
    pub y: f32,
    /// `true` = gehört dazu, `false` = gehört nicht dazu
    pub positive: bool,
}

/// Regler einer lokalen Anpassung. Wirken relativ zur globalen Entwicklung.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LocalAdjust {
    pub exposure: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub temperature: f32,
    pub tint: f32,
    pub saturation: f32,
    pub clarity: f32,
}

impl LocalAdjust {
    pub fn is_neutral(&self) -> bool {
        *self == LocalAdjust::default()
    }

    fn sanitized(&self) -> Self {
        let c = |v: f32, lim: f32| {
            if v.is_finite() {
                v.clamp(-lim, lim)
            } else {
                0.0
            }
        };
        Self {
            exposure: c(self.exposure, 4.0),
            contrast: c(self.contrast, 100.0),
            highlights: c(self.highlights, 100.0),
            shadows: c(self.shadows, 100.0),
            temperature: c(self.temperature, 100.0),
            tint: c(self.tint, 100.0),
            saturation: c(self.saturation, 100.0),
            clarity: c(self.clarity, 100.0),
        }
    }
}

impl MaskShape {
    fn sanitized(&self) -> Option<Self> {
        let f = |v: f32| v.is_finite();
        let ok = match self {
            MaskShape::Linear { x0, y0, x1, y1, .. } => {
                [x0, y0, x1, y1].iter().all(|v| f(**v))
                    && ((x1 - x0).abs() + (y1 - y0).abs()) > 1e-4
            }
            MaskShape::Radial {
                cx,
                cy,
                rx,
                ry,
                angle,
                feather,
                ..
            } => {
                [cx, cy, rx, ry, angle, feather].iter().all(|v| f(**v)) && *rx > 1e-4 && *ry > 1e-4
            }
            MaskShape::Brush { .. } => true,
            MaskShape::Color { samples, range, .. } => {
                !samples.is_empty()
                    && f(*range)
                    && samples
                        .iter()
                        .all(|s| f(s.x) && f(s.y) && s.lab.iter().all(|v| f(*v)))
            }
            // Ohne Längengrenze (anders als `repair::valid_id`): zu lange IDs
            // lehnt erst der Maskenspeicher beim Laden ab, die Form bleibt dann leer.
            MaskShape::Raster { mask_id, .. } => {
                !mask_id.is_empty()
                    && mask_id
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            }
        };
        if !ok {
            return None;
        }
        Some(match self {
            MaskShape::Radial {
                cx,
                cy,
                rx,
                ry,
                angle,
                feather,
                invert,
                subtract,
            } => MaskShape::Radial {
                cx: *cx,
                cy: *cy,
                rx: *rx,
                ry: *ry,
                angle: *angle,
                feather: feather.clamp(0.0, 1.0),
                invert: *invert,
                subtract: *subtract,
            },
            MaskShape::Brush { strokes, subtract } => MaskShape::Brush {
                strokes: strokes
                    .iter()
                    .filter(|s| !s.points.is_empty() && s.radius.is_finite() && s.radius > 0.0)
                    .map(|s| Stroke {
                        points: s
                            .points
                            .iter()
                            .copied()
                            .filter(|p| p[0].is_finite() && p[1].is_finite())
                            .collect(),
                        radius: s.radius.clamp(0.0005, 0.5),
                        feather: if s.feather.is_finite() {
                            s.feather.clamp(0.0, 1.0)
                        } else {
                            0.5
                        },
                        opacity: if s.opacity.is_finite() {
                            s.opacity.clamp(0.0, 1.0)
                        } else {
                            1.0
                        },
                        erase: s.erase,
                    })
                    .collect(),
                subtract: *subtract,
            },
            MaskShape::Color {
                samples,
                range,
                invert,
                subtract,
            } => MaskShape::Color {
                samples: samples.iter().take(32).copied().collect(),
                range: range.clamp(0.0, 100.0),
                invert: *invert,
                subtract: *subtract,
            },
            other => other.clone(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Crop {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Default for DevelopParams {
    fn default() -> Self {
        Self {
            version: CURRENT_VERSION,
            temperature: 0.0,
            tint: 0.0,
            exposure: 0.0,
            contrast: 0.0,
            highlights: 0.0,
            shadows: 0.0,
            whites: 0.0,
            blacks: 0.0,
            clarity: 0.0,
            vibrance: 0.0,
            saturation: 0.0,
            sharpen: 0.0,
            hsl: Hsl::default(),
            rotate: 0,
            flip_horizontal: false,
            straighten: 0.0,
            crop: None,
            masks: vec![],
            denoise: None,
            repairs: vec![],
        }
    }
}

impl DevelopParams {
    /// Begrenzt alle Werte auf ihre gültigen Bereiche. Schützt die Pipeline vor
    /// kaputten oder manipulierten Katalogeinträgen.
    pub fn sanitized(&self) -> Self {
        let c = |v: f32, lo: f32, hi: f32| if v.is_finite() { v.clamp(lo, hi) } else { 0.0 };
        let crop = self.crop.and_then(|k| {
            let x = c(k.x, 0.0, 1.0);
            let y = c(k.y, 0.0, 1.0);
            let w = c(k.w, 0.0, 1.0 - x);
            let h = c(k.h, 0.0, 1.0 - y);
            // Ein zu kleiner oder voller Zuschnitt ist keiner.
            let full = x <= 1e-4 && y <= 1e-4 && w >= 0.9999 && h >= 0.9999;
            (w > 0.01 && h > 0.01 && !full).then_some(Crop { x, y, w, h })
        });
        Self {
            version: self.version,
            temperature: c(self.temperature, -100.0, 100.0),
            tint: c(self.tint, -100.0, 100.0),
            exposure: c(self.exposure, -5.0, 5.0),
            contrast: c(self.contrast, -100.0, 100.0),
            highlights: c(self.highlights, -100.0, 100.0),
            shadows: c(self.shadows, -100.0, 100.0),
            whites: c(self.whites, -100.0, 100.0),
            blacks: c(self.blacks, -100.0, 100.0),
            clarity: c(self.clarity, -100.0, 100.0),
            vibrance: c(self.vibrance, -100.0, 100.0),
            saturation: c(self.saturation, -100.0, 100.0),
            sharpen: c(self.sharpen, 0.0, 150.0),
            hsl: Hsl {
                hue: self.hsl.hue.map(|v| c(v, -100.0, 100.0)),
                saturation: self.hsl.saturation.map(|v| c(v, -100.0, 100.0)),
                luminance: self.hsl.luminance.map(|v| c(v, -100.0, 100.0)),
            },
            rotate: self.rotate % 4,
            flip_horizontal: self.flip_horizontal,
            straighten: c(self.straighten, -45.0, 45.0),
            crop,
            denoise: self.denoise.as_ref().and_then(|d| {
                let ok = crate::repair::valid_id(&d.id);
                let amount = if d.amount.is_finite() {
                    d.amount.clamp(0.0, 100.0)
                } else {
                    100.0
                };
                (ok && amount > 0.0).then(|| DenoiseRef {
                    id: d.id.clone(),
                    amount,
                })
            }),
            masks: self
                .masks
                .iter()
                .take(32)
                .map(|m| MaskLayer {
                    id: m.id.clone(),
                    name: m.name.clone(),
                    enabled: m.enabled,
                    shapes: m.shapes.iter().filter_map(MaskShape::sanitized).collect(),
                    adjust: m.adjust.sanitized(),
                })
                .collect(),
            repairs: self
                .repairs
                .iter()
                .take(64)
                .map(|r| RepairLayer {
                    id: r.id.clone(),
                    name: r.name.clone(),
                    enabled: r.enabled,
                    shapes: r.shapes.iter().filter_map(MaskShape::sanitized).collect(),
                    result: r.result.clone().filter(|id| crate::repair::valid_id(id)),
                    result_key: r.result_key.clone(),
                })
                .collect(),
        }
    }

    pub fn is_default(&self) -> bool {
        self.sanitized() == DevelopParams::default()
    }

    /// IDs aller Raster-Masken, die diese Bearbeitung braucht.
    pub fn raster_mask_ids(&self) -> Vec<String> {
        self.masks
            .iter()
            .flat_map(|m| &m.shapes)
            .chain(self.repairs.iter().flat_map(|r| &r.shapes))
            .filter_map(|s| match s {
                MaskShape::Raster { mask_id, .. } => Some(mask_id.clone()),
                _ => None,
            })
            .collect()
    }

    /// Überträgt Einstellungen von `src` auf diese Bearbeitung (Kopieren/Einfügen,
    /// „Vorherige“). Nur die gewählten Gruppen; alles andere bleibt, wie es ist.
    ///
    /// Nie übertragen wird, was an Dateien des Ursprungsfotos hängt:
    /// KI-Rauschreduzierung, KI-Reparaturen und KI-Masken (Motiv, Himmel …).
    /// Masken aus Verlauf, Radial und Pinsel sind normiert und passen überall.
    pub fn transfer_from(&self, src: &DevelopParams, g: TransferGroups) -> DevelopParams {
        let mut out = self.clone();
        if g.basic {
            out.temperature = src.temperature;
            out.tint = src.tint;
            out.exposure = src.exposure;
            out.contrast = src.contrast;
            out.highlights = src.highlights;
            out.shadows = src.shadows;
            out.whites = src.whites;
            out.blacks = src.blacks;
            out.clarity = src.clarity;
            out.vibrance = src.vibrance;
            out.saturation = src.saturation;
            out.sharpen = src.sharpen;
            out.hsl = src.hsl;
        }
        if g.masks {
            let is_ai = |m: &MaskLayer| {
                m.shapes
                    .iter()
                    .any(|s| matches!(s, MaskShape::Raster { .. }))
            };
            // Eigene KI-Masken bleiben (gehören zu diesem Foto), die übrigen
            // werden durch die übertragbaren der Quelle ersetzt.
            out.masks = self
                .masks
                .iter()
                .filter(|m| is_ai(m))
                .chain(src.masks.iter().filter(|m| !is_ai(m)))
                .cloned()
                .collect();
        }
        if g.geometry {
            out.rotate = src.rotate;
            out.flip_horizontal = src.flip_horizontal;
            out.straighten = src.straighten;
            out.crop = src.crop;
        }
        out.sanitized()
    }

    /// Liest Parameter aus JSON; unbekannte oder fehlende Felder sind kein Fehler.
    pub fn from_json(v: &serde_json::Value) -> Self {
        serde_json::from_value::<DevelopParams>(v.clone())
            .unwrap_or_default()
            .sanitized()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfer_keeps_image_specific_parts() {
        let src = DevelopParams::from_json(&serde_json::json!({
            "exposure": 1.0, "contrast": 20.0, "temperature": 15.0, "rotate": 1,
            "denoise": { "id": "7-dn-1", "amount": 80.0 },
            "masks": [
                { "id": "a", "shapes": [{ "type": "radial", "cx": 0.5, "cy": 0.5, "rx": 0.2, "ry": 0.2 }],
                  "adjust": { "exposure": 0.5 } },
                { "id": "b", "shapes": [{ "type": "raster", "maskId": "7-subject-1", "kind": "subject" }],
                  "adjust": { "exposure": -0.5 } }
            ]
        }));
        assert!(
            src.denoise.is_some() && src.masks.len() == 2,
            "Testdaten gültig"
        );
        let dst = DevelopParams::from_json(&serde_json::json!({
            "exposure": -1.0, "shadows": 30.0, "rotate": 3,
            "denoise": { "id": "9-dn-1", "amount": 50.0 },
            "masks": [
                { "id": "eigen-ki", "shapes": [{ "type": "raster", "maskId": "9-sky-1", "kind": "sky" }] },
                { "id": "eigen-radial", "shapes": [{ "type": "radial", "cx": 0.1, "cy": 0.1, "rx": 0.1, "ry": 0.1 }] }
            ]
        }));

        let basic = dst.transfer_from(&src, TransferGroups::default());
        assert_eq!(
            (basic.exposure, basic.contrast, basic.temperature),
            (1.0, 20.0, 15.0)
        );
        assert_eq!(basic.shadows, 0.0, "Basis wird vollständig übernommen");
        assert_eq!(basic.rotate, 3, "Geometrie bleibt");
        assert_eq!(
            basic.denoise.as_ref().unwrap().id,
            "9-dn-1",
            "eigene Entrauschung bleibt"
        );
        assert_eq!(
            basic.masks.len(),
            2,
            "ohne Gruppe Masken bleiben sie unberührt"
        );

        let all = dst.transfer_from(
            &src,
            TransferGroups {
                basic: true,
                masks: true,
                geometry: true,
            },
        );
        assert_eq!(all.rotate, 1);
        let ids: Vec<&str> = all.masks.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["eigen-ki", "a"],
            "eigene KI-Maske bleibt, fremde nicht"
        );
        assert_eq!(all.denoise.as_ref().unwrap().id, "9-dn-1");
    }

    #[test]
    fn old_json_without_new_fields_still_loads() {
        let p = DevelopParams::from_json(&serde_json::json!({ "exposure": 0.5, "unbekannt": 1 }));
        assert_eq!(p.exposure, 0.5);
        assert_eq!(p.contrast, 0.0);
    }

    #[test]
    fn sanitizes() {
        let p = DevelopParams {
            exposure: 99.0,
            contrast: f32::NAN,
            rotate: 7,
            crop: Some(Crop {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0,
            }),
            ..Default::default()
        }
        .sanitized();
        assert_eq!(
            (p.exposure, p.contrast, p.rotate, p.crop),
            (5.0, 0.0, 3, None)
        );
        assert!(DevelopParams::default().is_default());
    }

    #[test]
    fn masks_roundtrip_through_json() {
        let json = serde_json::json!({
            "masks": [{
                "id": "m1", "name": "Himmel",
                "shapes": [
                    {"type": "linear", "x0": 0.5, "y0": 0.0, "x1": 0.5, "y1": 0.6},
                    {"type": "raster", "maskId": "abc-1", "kind": "sky", "subtract": true},
                    {"type": "raster", "maskId": "../../etc/passwd", "kind": "x"}
                ],
                "adjust": {"exposure": -0.7, "saturation": 20}
            }]
        });
        let p = DevelopParams::from_json(&json);
        assert_eq!(p.masks.len(), 1);
        assert!(p.masks[0].enabled, "Standard: aktiv");
        assert_eq!(p.masks[0].shapes.len(), 2, "unsichere Masken-ID verworfen");
        assert_eq!(p.masks[0].adjust.exposure, -0.7);
        assert_eq!(p.raster_mask_ids(), vec!["abc-1".to_string()]);
        let back: DevelopParams =
            serde_json::from_value(serde_json::to_value(&p).unwrap()).unwrap();
        assert_eq!(back, p);
    }
}
