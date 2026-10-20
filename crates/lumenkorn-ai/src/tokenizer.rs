//! CLIP-Tokenizer (Byte-Level-BPE), kompatibel mit `vocab.json`/`merges.txt`
//! im Hugging-Face-Format. Eigene Implementierung statt des `tokenizers`-Crates:
//! klein, ohne C-Abhängigkeiten und damit problemlos auf Android.

use crate::error::{Error, Result};
use lumenkorn_develop::i18n::tr_s;
use regex::Regex;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

pub const CONTEXT_LENGTH: usize = 77;
/// Längere Eingaben werden vor dem Zerlegen gekürzt.
const MAX_CHARS: usize = 1000;

pub struct ClipTokenizer {
    encoder: HashMap<String, i64>,
    ranks: HashMap<(String, String), usize>,
    byte_encoder: [char; 256],
    pattern: Regex,
    cache: Mutex<HashMap<String, Vec<String>>>,
    pub sot: i64,
    pub eot: i64,
}

impl ClipTokenizer {
    pub fn load(dir: &Path) -> Result<Self> {
        let vocab = std::fs::read_to_string(dir.join("vocab.json"))
            .map_err(|_| Error::ModelMissing("vocab.json".into()))?;
        let merges = std::fs::read_to_string(dir.join("merges.txt"))
            .map_err(|_| Error::ModelMissing("merges.txt".into()))?;
        Self::from_strings(&vocab, &merges)
    }

    pub fn from_strings(vocab_json: &str, merges_txt: &str) -> Result<Self> {
        let encoder: HashMap<String, i64> = serde_json::from_str(vocab_json)?;
        let ranks = merges_txt
            .lines()
            .filter(|l| !l.starts_with("#version") && !l.trim().is_empty())
            .enumerate()
            .filter_map(|(i, l)| {
                let mut it = l.split(' ');
                Some(((it.next()?.to_string(), it.next()?.to_string()), i))
            })
            .collect();
        let sot = *encoder.get("<|startoftext|>").ok_or_else(|| {
            Error::Tokenizer(tr_s("<|startoftext|> fehlt", "<|startoftext|> missing").into())
        })?;
        let eot = *encoder.get("<|endoftext|>").ok_or_else(|| {
            Error::Tokenizer(tr_s("<|endoftext|> fehlt", "<|endoftext|> missing").into())
        })?;
        let pattern = Regex::new(
            r"<\|startoftext\|>|<\|endoftext\|>|'s|'t|'re|'ve|'m|'ll|'d|[\p{L}]+|[\p{N}]|[^\s\p{L}\p{N}]+",
        )
        .map_err(|e| Error::Tokenizer(e.to_string()))?;
        Ok(Self {
            encoder,
            ranks,
            byte_encoder: bytes_to_unicode(),
            pattern,
            cache: Mutex::default(),
            sot,
            eot,
        })
    }

    /// Token-IDs ohne Start-/Endmarke (wie `SimpleTokenizer.encode`).
    pub fn encode(&self, text: &str) -> Vec<i64> {
        self.encode_max(text, usize::MAX)
    }

    /// Wie [`Self::encode`], hört aber nach `max` Tokens auf. Die Eingabe wird
    /// auf [`MAX_CHARS`] Zeichen begrenzt: Das Modell sieht ohnehin nur 77 Tokens,
    /// eine riesige Suchanfrage soll nicht erst vollständig zerlegt werden.
    fn encode_max(&self, text: &str, max: usize) -> Vec<i64> {
        let cleaned = text
            .chars()
            .take(MAX_CHARS)
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        let mut ids = Vec::new();
        for m in self.pattern.find_iter(&cleaned) {
            if ids.len() >= max {
                break;
            }
            let token: String = m
                .as_str()
                .bytes()
                .map(|b| self.byte_encoder[b as usize])
                .collect();
            for piece in self.bpe(&token) {
                if let Some(id) = self.encoder.get(&piece) {
                    ids.push(*id);
                }
            }
        }
        ids
    }

    /// Feste Länge 77 mit Start- und Endmarke, aufgefüllt mit der Endmarke.
    /// Liefert IDs und Aufmerksamkeitsmaske.
    pub fn encode_padded(&self, text: &str) -> (Vec<i64>, Vec<i64>) {
        let mut ids = vec![self.sot];
        let body = self.encode_max(text, CONTEXT_LENGTH - 2);
        ids.extend(body.into_iter().take(CONTEXT_LENGTH - 2));
        ids.push(self.eot);
        let mut mask = vec![1i64; ids.len()];
        ids.resize(CONTEXT_LENGTH, self.eot);
        mask.resize(CONTEXT_LENGTH, 0);
        (ids, mask)
    }

    fn bpe(&self, token: &str) -> Vec<String> {
        if let Some(hit) = self.cache.lock().unwrap().get(token) {
            return hit.clone();
        }
        let chars: Vec<char> = token.chars().collect();
        if chars.is_empty() {
            return vec![];
        }
        // Wortende-Markierung am letzten Zeichen
        let mut word: Vec<String> = chars[..chars.len() - 1]
            .iter()
            .map(|c| c.to_string())
            .collect();
        word.push(format!("{}</w>", chars[chars.len() - 1]));

        while word.len() > 1 {
            let best = (0..word.len() - 1)
                .filter_map(|i| {
                    self.ranks
                        .get(&(word[i].clone(), word[i + 1].clone()))
                        .map(|r| (*r, i))
                })
                .min();
            let Some((_, at)) = best else { break };
            let pair = (word[at].clone(), word[at + 1].clone());
            let mut merged = Vec::with_capacity(word.len());
            let mut i = 0;
            while i < word.len() {
                if i + 1 < word.len() && word[i] == pair.0 && word[i + 1] == pair.1 {
                    merged.push(format!("{}{}", pair.0, pair.1));
                    i += 2;
                } else {
                    merged.push(word[i].clone());
                    i += 1;
                }
            }
            word = merged;
        }
        let mut cache = self.cache.lock().unwrap();
        // Nicht unbegrenzt wachsen lassen (Suchanfragen mit vielen Fantasiewörtern)
        if cache.len() >= 50_000 {
            cache.clear();
        }
        cache.insert(token.to_string(), word.clone());
        word
    }
}

/// GPT-2/CLIP-Abbildung von Bytes auf druckbare Unicode-Zeichen.
fn bytes_to_unicode() -> [char; 256] {
    let mut bs: Vec<u32> = (b'!' as u32..=b'~' as u32)
        .chain(0xA1..=0xAC)
        .chain(0xAE..=0xFF)
        .collect();
    let mut cs = bs.clone();
    let mut n = 0;
    for b in 0..256u32 {
        if !bs.contains(&b) {
            bs.push(b);
            cs.push(256 + n);
            n += 1;
        }
    }
    let mut out = ['\0'; 256];
    for (b, c) in bs.into_iter().zip(cs) {
        out[b as usize] = char::from_u32(c).expect("gültiges Zeichen");
    }
    out
}
