//! Liest PyTorch-Gewichte im alten Speicherformat (vor PyTorch 1.6) ohne
//! PyTorch: eine Folge von Pickles, gefolgt von den rohen Tensor-Daten.
//!
//! Unterstützt genau das, was Zustandswörterbücher (state_dict) brauchen:
//! ein Wörterbuch Name → Tensor mit float32-Daten.

use crate::error::{Error, Result};
use std::collections::HashMap;
use std::io::{Cursor, Read};

const MAGIC: u128 = 0x1950a86a20f9469cfc6c;

#[derive(Debug, Clone)]
pub struct Tensor {
    pub shape: Vec<usize>,
    pub data: Vec<f32>,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
enum V {
    None,
    Bool(bool),
    Int(i128),
    Float(f64),
    Str(String),
    Tuple(Vec<V>),
    List(Vec<V>),
    Dict(Vec<(V, V)>),
    Global(String),
    /// Verweis auf einen Speicherblock: (Schlüssel, Elementtyp, Anzahl)
    Storage(String, String, usize),
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
    Error::ModelFormat(format!("Gewichtsdatei: {}", msg.into()))
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
            .map_err(|_| bad("unerwartetes Ende"))?;
        Ok(b[0])
    }
    fn bytes(&mut self, n: usize) -> Result<Vec<u8>> {
        let mut b = vec![0u8; n];
        self.r
            .read_exact(&mut b)
            .map_err(|_| bad("unerwartetes Ende"))?;
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
        self.stack.pop().ok_or_else(|| bad("leerer Stapel"))
    }
    fn pop_mark(&mut self) -> Result<Vec<V>> {
        let pos = self
            .stack
            .iter()
            .rposition(|v| matches!(v, V::Mark))
            .ok_or_else(|| bad("MARK fehlt"))?;
        let items = self.stack.split_off(pos + 1);
        self.stack.pop();
        Ok(items)
    }
    fn top_mut(&mut self) -> Result<&mut V> {
        self.stack.last_mut().ok_or_else(|| bad("leerer Stapel"))
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
                0x88 => p.stack.push(V::Bool(true)),
                0x89 => p.stack.push(V::Bool(false)),
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
                    let b = p.bytes(8)?;
                    p.stack
                        .push(V::Float(f64::from_be_bytes(b.try_into().unwrap())));
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
                    let at = p.stack.len().checked_sub(n).ok_or_else(|| bad("Tupel"))?;
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
                        return Err(bad("persistente ID"));
                    };
                    match (t.get(1), t.get(2), t.get(4)) {
                        (Some(V::Global(ty)), Some(V::Str(key)), Some(V::Int(n))) => p
                            .stack
                            .push(V::Storage(key.clone(), ty.clone(), *n as usize)),
                        _ => return Err(bad("unbekannte persistente ID")),
                    }
                }
                b'R' => {
                    let args = p.pop()?;
                    let func = p.pop()?;
                    let V::Global(f) = func else {
                        return Err(bad("REDUCE ohne Funktion"));
                    };
                    let V::Tuple(a) = args else {
                        return Err(bad("REDUCE ohne Argumente"));
                    };
                    let v = if f.starts_with("torch._utils._rebuild_tensor") {
                        let key = match a.first() {
                            Some(V::Storage(k, ty, _)) if ty.ends_with("FloatStorage") => k.clone(),
                            Some(V::Storage(_, ty, _)) => {
                                return Err(bad(format!("nicht unterstützter Typ {ty}")));
                            }
                            _ => return Err(bad("Tensor ohne Speicher")),
                        };
                        let ints = |v: Option<&V>| -> Result<Vec<usize>> {
                            match v {
                                Some(V::Tuple(x)) => x
                                    .iter()
                                    .map(|i| match i {
                                        V::Int(n) => Ok(*n as usize),
                                        _ => Err(bad("Form")),
                                    })
                                    .collect(),
                                _ => Err(bad("Form")),
                            }
                        };
                        let offset = match a.get(1) {
                            Some(V::Int(n)) => *n as usize,
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
                    return Err(bad(format!(
                        "Pickle-Befehl 0x{other:02x} nicht unterstützt"
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
        _ => return Err(bad("kein PyTorch-Legacy-Format")),
    }
    Pickle::run(&mut cur)?;
    Pickle::run(&mut cur)?;
    let obj = Pickle::run(&mut cur)?;
    let keys = Pickle::run(&mut cur)?;
    let V::List(keys) = keys else {
        return Err(bad("Speicherliste fehlt"));
    };

    // Rohdaten der Speicherblöcke in der angegebenen Reihenfolge
    let mut storages: HashMap<String, Vec<f32>> = HashMap::new();
    for k in keys {
        let V::Str(k) = k else {
            return Err(bad("Speicherschlüssel"));
        };
        let mut nb = [0u8; 8];
        cur.read_exact(&mut nb).map_err(|_| bad("Speichergröße"))?;
        let n = i64::from_le_bytes(nb) as usize;
        let mut raw = vec![0u8; n * 4];
        cur.read_exact(&mut raw)
            .map_err(|_| bad("Speicherdaten unvollständig"))?;
        storages.insert(
            k,
            raw.chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect(),
        );
    }

    let V::Dict(entries) = obj else {
        return Err(bad("kein Wörterbuch"));
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
            .ok_or_else(|| bad(format!("Speicher {key} fehlt")))?;
        let n: usize = shape.iter().product();
        let mut data = Vec::with_capacity(n);
        let mut idx = vec![0usize; shape.len()];
        for _ in 0..n {
            let pos = offset + idx.iter().zip(&stride).map(|(i, s)| i * s).sum::<usize>();
            data.push(*st.get(pos).ok_or_else(|| bad("Index außerhalb"))?);
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
