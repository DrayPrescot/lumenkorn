//! Semantische Suche: alle Bild-Embeddings im Speicher, Suche per Skalarprodukt.
//! 50 000 Bilder × 512 Werte ≈ 100 MB und unter 20 ms pro Anfrage – ein
//! spezieller Vektorindex lohnt sich erst bei deutlich größeren Bibliotheken.

use crate::clip::dot;
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub struct EmbeddingIndex {
    dim: usize,
    ids: Vec<i64>,
    data: Vec<f32>,
    /// ID → Position; ohne sie wäre das Füllen mit n Bildern O(n²)
    slots: HashMap<i64, usize>,
}

impl EmbeddingIndex {
    pub fn new(dim: usize) -> Self {
        Self {
            dim,
            ..Default::default()
        }
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// Fügt ein Embedding hinzu oder ersetzt das vorhandene.
    pub fn upsert(&mut self, id: i64, v: &[f32]) {
        if self.dim == 0 {
            self.dim = v.len();
        }
        if v.len() != self.dim {
            log::warn!(
                "Embedding {id} hat {} statt {} Werte – übersprungen",
                v.len(),
                self.dim
            );
            return;
        }
        match self.slots.get(&id) {
            Some(&i) => self.data[i * self.dim..(i + 1) * self.dim].copy_from_slice(v),
            None => {
                self.slots.insert(id, self.ids.len());
                self.ids.push(id);
                self.data.extend_from_slice(v);
            }
        }
    }

    pub fn remove(&mut self, id: i64) {
        if let Some(i) = self.slots.remove(&id) {
            self.ids.swap_remove(i);
            let last = self.ids.len();
            if i != last {
                self.slots.insert(self.ids[i], i);
                let (a, b) = self.data.split_at_mut(last * self.dim);
                a[i * self.dim..(i + 1) * self.dim].copy_from_slice(&b[..self.dim]);
            }
            self.data.truncate(last * self.dim);
        }
    }

    /// Die `limit` ähnlichsten Bilder mit Kosinus ≥ `min_score`, absteigend.
    pub fn search(&self, query: &[f32], limit: usize, min_score: f32) -> Vec<(i64, f32)> {
        self.search_in(query, limit, min_score, None)
    }

    /// Wie [`search`](Self::search), aber nur unter den Bildern in `allowed`
    /// (z. B. aktueller Ordner). Gefiltert wird vor dem Kürzen auf `limit` –
    /// sonst verdrängen bessere Treffer außerhalb des Bereichs die darin.
    pub fn search_in(
        &self,
        query: &[f32],
        limit: usize,
        min_score: f32,
        allowed: Option<&HashSet<i64>>,
    ) -> Vec<(i64, f32)> {
        if query.len() != self.dim || self.dim == 0 {
            return vec![];
        }
        let mut hits: Vec<(i64, f32)> = self
            .ids
            .iter()
            .zip(self.data.chunks_exact(self.dim))
            .filter(|(id, _)| allowed.is_none_or(|a| a.contains(id)))
            .map(|(id, v)| (*id, dot(query, v)))
            .filter(|(_, s)| *s >= min_score)
            .collect();
        // Nur die besten `limit` sortieren, nicht alle 50 000
        if hits.len() > limit && limit > 0 {
            hits.select_nth_unstable_by(limit - 1, |a, b| b.1.total_cmp(&a.1));
            hits.truncate(limit);
        }
        hits.sort_by(|a, b| b.1.total_cmp(&a.1));
        hits.truncate(limit);
        hits
    }
}

/// Embeddings werden als Little-Endian-f32 in der Datenbank gespeichert.
pub fn to_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

pub fn from_bytes(b: &[u8]) -> Vec<f32> {
    b.as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_and_remove() {
        let mut idx = EmbeddingIndex::new(2);
        idx.upsert(1, &[1.0, 0.0]);
        idx.upsert(2, &[0.0, 1.0]);
        idx.upsert(3, &[0.6, 0.8]);
        let hits = idx.search(&[1.0, 0.0], 10, 0.5);
        assert_eq!(hits.iter().map(|h| h.0).collect::<Vec<_>>(), vec![1, 3]);
        idx.remove(1);
        assert_eq!(idx.len(), 2);
        assert_eq!(idx.search(&[1.0, 0.0], 10, 0.5)[0].0, 3);
        idx.upsert(2, &[1.0, 0.0]);
        assert_eq!(idx.search(&[1.0, 0.0], 1, 0.0)[0].0, 2);
        // 3 ist beim Entfernen nach vorn gerückt: Ersetzen statt Verdoppeln
        idx.upsert(3, &[0.0, -1.0]);
        assert_eq!(idx.len(), 2);
        assert!(idx.search(&[0.6, 0.8], 10, 0.5).iter().all(|h| h.0 != 3));
        assert_eq!(from_bytes(&to_bytes(&[1.5, -2.0])), vec![1.5, -2.0]);
    }

    #[test]
    fn search_restricted_to_scope() {
        // 3000 globale Treffer, die alle besser passen als die beiden im Ordner
        let mut idx = EmbeddingIndex::new(2);
        for id in 0..3000 {
            idx.upsert(id, &[1.0, 0.0]);
        }
        idx.upsert(10_001, &[0.6, 0.8]);
        idx.upsert(10_002, &[0.0, 1.0]);
        let q = [1.0, 0.0];
        // Ungefiltert sind die Ordnerbilder nicht unter den besten 2000
        assert!(idx.search(&q, 2000, -1.0).iter().all(|h| h.0 < 10_000));
        let folder: HashSet<i64> = [10_001, 10_002, 99_999].into();
        let hits = idx.search_in(&q, 2000, -1.0, Some(&folder));
        assert_eq!(
            hits.iter().map(|h| h.0).collect::<Vec<_>>(),
            vec![10_001, 10_002]
        );
        // Bester Treffer im Bereich, nicht global
        assert!((hits[0].1 - 0.6).abs() < 1e-6);
        // Leerer Bereich → nichts
        assert!(
            idx.search_in(&q, 10, -1.0, Some(&HashSet::new()))
                .is_empty()
        );
    }
}
