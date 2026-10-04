//! Zero-Shot-Verschlagwortung: vergleicht ein Bild-Embedding mit den
//! vorberechneten Text-Embeddings aller Konzepte.

use crate::clip::{Clip, dot};
use crate::error::Result;
use crate::vocab::{Group, TEMPLATES};

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
    /// Berechnet die Text-Embeddings des Vokabulars (einmal beim Start, ~1 s).
    pub fn new(clip: &Clip, groups: Vec<Group>) -> Result<Self> {
        let prompts = |phrase: &str| {
            TEMPLATES
                .iter()
                .map(|t| t.replace("{}", phrase))
                .collect::<Vec<_>>()
        };
        let mut encoded = Vec::new();
        for g in groups {
            let mut embeddings = vec![clip.embed_concept(&prompts(g.neutral))?];
            for c in &g.concepts {
                embeddings.push(clip.embed_concept(&prompts(c.phrase))?);
            }
            encoded.push(EncodedGroup {
                group: g,
                embeddings,
            });
        }
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
