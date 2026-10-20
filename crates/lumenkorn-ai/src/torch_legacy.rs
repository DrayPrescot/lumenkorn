//! Liest PyTorch-Gewichte im alten Speicherformat (vor PyTorch 1.6) ohne
//! PyTorch: eine Folge von Pickles, gefolgt von den rohen Tensor-Daten.
//!
//! Unterstützt genau das, was Zustandswörterbücher (state_dict) brauchen:
//! ein Wörterbuch Name → Tensor mit float32-Daten.

use crate::error::{Error, Result};
use lumenkorn_develop::{i18n::tr_s, tr};
use std::collections::HashMap;
use std::io::{Cursor, Read};

const MAGIC: u128 = 0x1950a86a20f9469cfc6c;

#[derive(Debug, Clone)]
pub struct Tensor {
    pub shape: Vec<usize>,
    pub data: Vec<f32>,
}

/// Pickle-Werte. Bool und Float tragen keinen Wert – ein state_dict braucht
/// sie nur als Platzhalter auf dem Stapel.
#[derive(Debug, Clone)]
enum V {
    None,
    Bool,
    Int(i128),
    Float,
    Str(String),
    Tuple(Vec<V>),
    List(Vec<V>),
    Dict(Vec<(V, V)>),
    Global(String),
    /// Verweis auf einen Speicherblock: (Schlüssel, Elementtyp)
    Storage(String, String),
    /// Noch nicht befüllter Tensor
    Tensor {
        key: String,
        offset: usize,
        shape: Vec<usize>,
        stride: Vec<usize>,
    },
    Mark,
}

fn bad(msg: impl Into<String>) -> Error {
    Error::ModelFormat(format!(
        "{}: {}",
        tr_s("Gewichtsdatei", "Weights file"),
        msg.into()
    ))
}

struct Pickle<'r, 'b> {
    r: &'r mut Cursor<&'b [u8]>,
    stack: Vec<V>,
    memo: HashMap<u32, V>,
}

impl<'r, 'b> Pickle<'r, 'b> {
    fn u8(&mut self) -> Result<u8> {
        let mut b = [0u8; 1];
        self.r
            .read_exact(&mut b)
            .map_err(|_| bad(tr_s("unerwartetes Ende", "unexpected end")))?;
        Ok(b[0])
    }
    fn bytes(&mut self, n: usize) -> Result<Vec<u8>> {
        // Länge gegen den Rest prüfen, bevor Speicher angelegt wird: eine
        // gefälschte Längenangabe (bis 4 GB) darf keine Allokation auslösen.
        let rest =
            self.r.get_ref().len() as u64 - self.r.position().min(self.r.get_ref().len() as u64);
        if n as u64 > rest {
            return Err(bad(tr_s("unerwartetes Ende", "unexpected end")));
        }
        let mut b = vec![0u8; n];
        self.r
            .read_exact(&mut b)
            .map_err(|_| bad(tr_s("unerwartetes Ende", "unexpected end")))?;
        Ok(b)
    }
    fn u16(&mut self) -> Result<u16> {
        let b = self.bytes(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Result<u32> {
        let b = self.bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn line(&mut self) -> Result<String> {
        let mut s = Vec::new();
        loop {
            let c = self.u8()?;
            if c == b'\n' {
                break;
            }
            s.push(c);
        }
        Ok(String::from_utf8_lossy(&s).into_owned())
    }
    fn pop(&mut self) -> Result<V> {
        self.stack
            .pop()
            .ok_or_else(|| bad(tr_s("leerer Stapel", "empty stack")))
    }
    fn pop_mark(&mut self) -> Result<Vec<V>> {
        let pos = self
            .stack
            .iter()
            .rposition(|v| matches!(v, V::Mark))
            .ok_or_else(|| bad(tr_s("MARK fehlt", "MARK missing")))?;
        let items = self.stack.split_off(pos + 1);
        self.stack.pop();
        Ok(items)
    }
    fn top_mut(&mut self) -> Result<&mut V> {
        self.stack
            .last_mut()
            .ok_or_else(|| bad(tr_s("leerer Stapel", "empty stack")))
    }

    fn run(r: &'r mut Cursor<&'b [u8]>) -> Result<V> {
        let mut p = Pickle {
            r,
            stack: Vec::new(),
            memo: HashMap::new(),
        };
        loop {
            let op = p.u8()?;
            match op {
                0x80 => {
                    p.u8()?;
                } // PROTO
                b'.' => return p.pop(), // STOP
                b'N' => p.stack.push(V::None),
                0x88 | 0x89 => p.stack.push(V::Bool), // NEWTRUE, NEWFALSE
                b'J' => {
                    let v = p.u32()? as i32;
                    p.stack.push(V::Int(v as i128))
                }
                b'K' => {
                    let v = p.u8()?;
                    p.stack.push(V::Int(v as i128))
                }
                b'M' => {
                    let v = p.u16()?;
                    p.stack.push(V::Int(v as i128))
                }
                0x8a => {
                    // LONG1
                    let n = p.u8()? as usize;
                    // Mehr als 15 Bytes passen nicht in i128 (Überlauf beim Schieben)
                    if n > 15 {
                        return Err(bad(tr_s("Zahl zu groß", "number too large")));
                    }
                    let b = p.bytes(n)?;
                    let mut v: i128 = 0;
                    for (i, x) in b.iter().enumerate() {
                        v |= (*x as i128) << (8 * i);
                    }
                    if n > 0 && b[n - 1] & 0x80 != 0 {
                        v -= 1i128 << (8 * n);
                    }
                    p.stack.push(V::Int(v));
                }
                b'G' => {
                    p.bytes(8)?;
                    p.stack.push(V::Float);
                }
                b'X' => {
                    let n = p.u32()? as usize;
                    let b = p.bytes(n)?;
                    p.stack
                        .push(V::Str(String::from_utf8_lossy(&b).into_owned()));
                }
                0x8c => {
                    let n = p.u8()? as usize;
                    let b = p.bytes(n)?;
                    p.stack
                        .push(V::Str(String::from_utf8_lossy(&b).into_owned()));
                }
                b'c' => {
                    let module = p.line()?;
                    let name = p.line()?;
                    p.stack.push(V::Global(format!("{module}.{name}")));
                }
                b'(' => p.stack.push(V::Mark),
                b')' => p.stack.push(V::Tuple(vec![])),
                b't' => {
                    let items = p.pop_mark()?;
                    p.stack.push(V::Tuple(items));
                }
                0x85..=0x87 => {
                    let n = (op - 0x84) as usize;
                    let at = p
                        .stack
                        .len()
                        .checked_sub(n)
                        .ok_or_else(|| bad(tr_s("Tupel", "tuple")))?;
                    let items = p.stack.split_off(at);
                    p.stack.push(V::Tuple(items));
                }
                b']' => p.stack.push(V::List(vec![])),
                b'}' => p.stack.push(V::Dict(vec![])),
                b'a' => {
                    let v = p.pop()?;
                    if let V::List(l) = p.top_mut()? {
                        l.push(v);
                    }
                }
                b'e' => {
                    let items = p.pop_mark()?;
                    if let V::List(l) = p.top_mut()? {
                        l.extend(items);
                    }
                }
                b's' => {
                    let v = p.pop()?;
                    let k = p.pop()?;
                    if let V::Dict(d) = p.top_mut()? {
                        d.push((k, v));
                    }
                }
                b'u' => {
                    let items = p.pop_mark()?;
                    if let V::Dict(d) = p.top_mut()? {
                        let mut it = items.into_iter();
                        while let (Some(k), Some(v)) = (it.next(), it.next()) {
                            d.push((k, v));
                        }
                    }
                }
                b'q' => {
                    let i = p.u8()? as u32;
                    let v = p.stack.last().cloned().ok_or_else(|| bad("BINPUT"))?;
                    p.memo.insert(i, v);
                }
                b'r' => {
                    let i = p.u32()?;
                    let v = p.stack.last().cloned().ok_or_else(|| bad("LONG_BINPUT"))?;
                    p.memo.insert(i, v);
                }
                b'h' => {
                    let i = p.u8()? as u32;
                    let v = p.memo.get(&i).cloned().ok_or_else(|| bad("BINGET"))?;
                    p.stack.push(v);
                }
                b'j' => {
                    let i = p.u32()?;
                    let v = p.memo.get(&i).cloned().ok_or_else(|| bad("LONG_BINGET"))?;
                    p.stack.push(v);
                }
                b'Q' => {
                    // BINPERSID: ('storage', Typ, Schlüssel, Ort, Anzahl, View)
                    let pid = p.pop()?;
                    let V::Tuple(t) = pid else {
                        return Err(bad(tr_s("persistente ID", "persistent ID")));
                    };
                    match (t.get(1), t.get(2), t.get(4)) {
                        // Die Anzahl (t[4]) muss eine Zahl sein, gebraucht wird
                        // sie nicht: die Größe ergibt sich aus der Form.
                        (Some(V::Global(ty)), Some(V::Str(key)), Some(V::Int(_))) => {
                            p.stack.push(V::Storage(key.clone(), ty.clone()))
                        }
                        _ => {
                            return Err(bad(tr_s(
                                "unbekannte persistente ID",
                                "unknown persistent ID",
                            )));
                        }
                    }
                }
                b'R' => {
                    let args = p.pop()?;
                    let func = p.pop()?;
                    let V::Global(f) = func else {
                        return Err(bad(tr_s("REDUCE ohne Funktion", "REDUCE without function")));
                    };
                    let V::Tuple(a) = args else {
                        return Err(bad(tr_s(
                            "REDUCE ohne Argumente",
                            "REDUCE without arguments",
                        )));
                    };
                    let v = if f.starts_with("torch._utils._rebuild_tensor") {
                        let key = match a.first() {
                            Some(V::Storage(k, ty)) if ty.ends_with("FloatStorage") => k.clone(),
                            Some(V::Storage(_, ty)) => {
                                return Err(bad(tr!(
                                    "nicht unterstützter Typ {ty}",
                                    "unsupported type {ty}"
                                )));
                            }
                            _ => {
                                return Err(bad(tr_s(
                                    "Tensor ohne Speicher",
                                    "tensor without storage",
                                )));
                            }
                        };
                        // Negative oder absurd große Werte (Form, Schritte, Anfang)
                        // gehören nicht in ein gültiges state_dict
                        let ints = |v: Option<&V>| -> Result<Vec<usize>> {
                            match v {
                                Some(V::Tuple(x)) => x
                                    .iter()
                                    .map(|i| match i {
                                        V::Int(n) => usize::try_from(*n)
                                            .map_err(|_| bad(tr_s("Form", "shape"))),
                                        _ => Err(bad(tr_s("Form", "shape"))),
                                    })
                                    .collect(),
                                _ => Err(bad(tr_s("Form", "shape"))),
                            }
                        };
                        let offset = match a.get(1) {
                            Some(V::Int(n)) => {
                                usize::try_from(*n).map_err(|_| bad(tr_s("Anfang", "offset")))?
                            }
                            _ => 0,
                        };
                        V::Tensor {
                            key,
                            offset,
                            shape: ints(a.get(2))?,
                            stride: ints(a.get(3))?,
                        }
                    } else if f == "collections.OrderedDict" {
                        V::Dict(vec![])
                    } else {
                        V::None
                    };
                    p.stack.push(v);
                }
                b'b' => {
                    // BUILD: Zustand setzen – für OrderedDict/Tensoren ohne Bedeutung
                    p.pop()?;
                }
                other => {
                    return Err(bad(tr!(
                        "Pickle-Befehl 0x{other:02x} nicht unterstützt",
                        "pickle opcode 0x{other:02x} not supported"
                    )));
                }
            }
        }
    }
}

/// Lädt ein state_dict (Name → float32-Tensor).
pub fn load_state_dict(bytes: &[u8]) -> Result<Vec<(String, Tensor)>> {
    let mut cur = Cursor::new(bytes);
    // 1. Magische Zahl, 2. Protokollversion, 3. Systeminfo, 4. Objekt, 5. Speicherliste
    match Pickle::run(&mut cur)? {
        V::Int(n) if n as u128 == MAGIC => {}
        _ => {
            return Err(bad(tr_s(
                "kein PyTorch-Legacy-Format",
                "not a legacy PyTorch format",
            )));
        }
    }
    Pickle::run(&mut cur)?;
    Pickle::run(&mut cur)?;
    let obj = Pickle::run(&mut cur)?;
    let keys = Pickle::run(&mut cur)?;
    let V::List(keys) = keys else {
        return Err(bad(tr_s("Speicherliste fehlt", "storage list missing")));
    };

    // Rohdaten der Speicherblöcke in der angegebenen Reihenfolge
    let mut storages: HashMap<String, Vec<f32>> = HashMap::new();
    for k in keys {
        let V::Str(k) = k else {
            return Err(bad(tr_s("Speicherschlüssel", "storage key")));
        };
        let mut nb = [0u8; 8];
        cur.read_exact(&mut nb)
            .map_err(|_| bad(tr_s("Speichergröße", "storage size")))?;
        // Anzahl gegen die restlichen Daten prüfen, bevor Speicher angelegt wird
        let rest = (bytes.len() as u64).saturating_sub(cur.position());
        let n = usize::try_from(i64::from_le_bytes(nb))
            .ok()
            .and_then(|n| n.checked_mul(4).map(|b| (n, b)))
            .filter(|(_, b)| *b as u64 <= rest)
            .map(|(n, _)| n)
            .ok_or_else(|| {
                bad(tr_s(
                    "Speicherdaten unvollständig",
                    "storage data incomplete",
                ))
            })?;
        let mut raw = vec![0u8; n * 4];
        cur.read_exact(&mut raw).map_err(|_| {
            bad(tr_s(
                "Speicherdaten unvollständig",
                "storage data incomplete",
            ))
        })?;
        storages.insert(
            k,
            raw.as_chunks::<4>()
                .0
                .iter()
                .map(|c| f32::from_le_bytes(*c))
                .collect(),
        );
    }

    let V::Dict(entries) = obj else {
        return Err(bad(tr_s("kein Wörterbuch", "not a dictionary")));
    };
    let mut out = Vec::with_capacity(entries.len());
    for (k, v) in entries {
        let (
            V::Str(name),
            V::Tensor {
                key,
                offset,
                shape,
                stride,
            },
        ) = (k, v)
        else {
            continue;
        };
        let st = storages
            .get(&key)
            .ok_or_else(|| bad(tr!("Speicher {key} fehlt", "storage {key} missing")))?;
        // Elementzahl ohne Überlauf; mehr als ein Speicherblock je Tensor
        // wäre nur mit Schritt 0 möglich und hat in Gewichten nichts verloren
        let n = shape
            .iter()
            .try_fold(1usize, |a, d| a.checked_mul(*d))
            .filter(|n| *n <= st.len())
            .ok_or_else(|| bad(tr_s("Form", "shape")))?;
        let mut data = Vec::with_capacity(n);
        let mut idx = vec![0usize; shape.len()];
        for _ in 0..n {
            let pos = idx.iter().zip(&stride).fold(offset, |acc, (i, s)| {
                acc.saturating_add(i.saturating_mul(*s))
            });
            data.push(
                *st.get(pos)
                    .ok_or_else(|| bad(tr_s("Index außerhalb", "index out of range")))?,
            );
            for d in (0..shape.len()).rev() {
                idx[d] += 1;
                if idx[d] < shape[d] {
                    break;
                }
                idx[d] = 0;
            }
        }
        out.push((name, Tensor { shape, data }));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn magic_pickles() -> Vec<u8> {
        let mut b = vec![0x80, 2, 0x8a, 10];
        b.extend_from_slice(&MAGIC.to_le_bytes()[..10]);
        b.push(b'.');
        b.extend_from_slice(b"N.N."); // Protokollversion, Systeminfo
        b.extend_from_slice(b"}."); // leeres Wörterbuch
        b.extend_from_slice(&[b']', b'(', 0x8c, 1, b'k', b'e', b'.']); // Schlüssel „k“
        b
    }

    #[test]
    fn rejects_forged_lengths_without_allocating() {
        // Zeichenkette mit 4-GB-Länge, aber nur wenigen Bytes Daten
        let mut b = vec![b'X'];
        b.extend_from_slice(&u32::MAX.to_le_bytes());
        b.extend_from_slice(b"abc.");
        assert!(Pickle::run(&mut Cursor::new(b.as_slice())).is_err());

        // LONG1 mit mehr Bytes, als in i128 passen
        let mut b = vec![0x8a, 255];
        b.extend_from_slice(&[1u8; 255]);
        b.push(b'.');
        assert!(Pickle::run(&mut Cursor::new(b.as_slice())).is_err());

        // Speicherblock mit absurder bzw. negativer Elementzahl
        for n in [i64::MAX, i64::MAX / 4 + 1, -1] {
            let mut b = magic_pickles();
            b.extend_from_slice(&n.to_le_bytes());
            assert!(load_state_dict(&b).is_err(), "{n}");
        }
        // Gutartig: ein leerer Block ist in Ordnung
        let mut b = magic_pickles();
        b.extend_from_slice(&0i64.to_le_bytes());
        assert!(load_state_dict(&b).unwrap().is_empty());
    }
}
