//! Zero-Shot-Verschlagwortung: vergleicht ein Bild-Embedding mit den
//! vorberechneten Text-Embeddings aller Konzepte.

use crate::clip::{Clip, dot, mean_normalized};
use crate::error::Result;
use crate::vocab::{Group, TEMPLATES};

/// Texte je Lauf des Textmodells beim Start (77 Token × 64 – klein genug
/// für jeden Rechner, groß genug, dass der Aufwand pro Lauf kaum zählt).
const TEXT_BATCH: usize = 64;

/// CLIP-Temperatur: Kosinus-Ähnlichkeiten × 100 ergeben die Logits.
const LOGIT_SCALE: f32 = 100.0;

#[derive(Debug, Clone, PartialEq)]
pub struct TagSuggestion {
    pub tag: String,
    pub group: String,
    /// Wahrscheinlichkeit innerhalb der Gruppe, 0…1
    pub confidence: f32,
}

struct EncodedGroup {
    group: Group,
    /// Index 0 = neutral, danach die Konzepte in Reihenfolge
    embeddings: Vec<Vec<f32>>,
}

pub struct Tagger {
    groups: Vec<EncodedGroup>,
}

impl Tagger {
    /// Berechnet die Text-Embeddings des Vokabulars (einmal beim Start).
    /// Alle Formulierungen aller Konzepte laufen in großen Stapeln durchs
    /// Textmodell statt je Konzept einzeln (80 Läufe mit je 3 Texten) – der
    /// feste Aufwand pro Lauf fällt so nur wenige Male an.
    pub fn new(clip: &Clip, groups: Vec<Group>) -> Result<Self> {
        let per = TEMPLATES.len();
        let mut texts: Vec<String> = Vec::new();
        for g in &groups {
            for phrase in std::iter::once(g.neutral).chain(g.concepts.iter().map(|c| c.phrase)) {
                texts.extend(TEMPLATES.iter().map(|t| t.replace("{}", phrase)));
            }
        }
        let mut rows = Vec::with_capacity(texts.len());
        for batch in texts.chunks(TEXT_BATCH) {
            let refs: Vec<&str> = batch.iter().map(String::as_str).collect();
            rows.extend(clip.embed_texts(&refs)?);
        }
        // Reihenfolge wie oben: je Gruppe neutral, dann die Konzepte
        let mut concepts = rows.chunks(per).map(mean_normalized);
        let encoded = groups
            .into_iter()
            .map(|group| EncodedGroup {
                embeddings: concepts.by_ref().take(group.concepts.len() + 1).collect(),
                group,
            })
            .collect();
        Ok(Self { groups: encoded })
    }

    pub fn tag(&self, image_embedding: &[f32]) -> Vec<TagSuggestion> {
        let mut out = Vec::new();
        for eg in &self.groups {
            let logits: Vec<f32> = eg
                .embeddings
                .iter()
                .map(|e| dot(e, image_embedding) * LOGIT_SCALE)
                .collect();
            let probs = softmax(&logits);
            let neutral = probs[0];
            let mut ranked: Vec<(usize, f32)> = probs.iter().copied().enumerate().skip(1).collect();
            ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
            for (i, p) in ranked.into_iter().take(eg.group.max_tags) {
                // Muss die Schwelle erreichen und wahrscheinlicher sein als „nichts davon“.
                if p < eg.group.min_p || p < neutral {
                    break;
                }
                out.push(TagSuggestion {
                    tag: eg.group.concepts[i - 1].name().to_string(),
                    group: eg.group.name.to_string(),
                    confidence: p,
                });
            }
        }
        out
    }
}

fn softmax(x: &[f32]) -> Vec<f32> {
    let max = x.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let e: Vec<f32> = x.iter().map(|v| (v - max).exp()).collect();
    let s: f32 = e.iter().sum();
    e.into_iter().map(|v| v / s).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn softmax_sums_to_one() {
        let p = softmax(&[1.0, 2.0, 3.0]);
        assert!((p.iter().sum::<f32>() - 1.0).abs() < 1e-6);
        assert!(p[2] > p[1] && p[1] > p[0]);
    }
}
