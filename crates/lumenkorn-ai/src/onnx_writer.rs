//! Minimaler ONNX-Schreiber: erzeugt ein ModelProto als Protobuf-Bytes, ohne
//! Protobuf-Bibliothek. Reicht für reine Faltungsnetze (Conv, ConvTranspose,
//! Relu, Add, Concat).
//!
//! Feldnummern laut onnx.proto (proto2, wiederholte Felder ungepackt).

#[derive(Default)]
struct Buf(Vec<u8>);

impl Buf {
    fn varint(&mut self, mut v: u64) {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                self.0.push(b);
                break;
            }
            self.0.push(b | 0x80);
        }
    }
    fn tag(&mut self, field: u32, wire: u8) {
        self.varint(((field as u64) << 3) | wire as u64);
    }
    fn int(&mut self, field: u32, v: i64) {
        self.tag(field, 0);
        self.varint(v as u64);
    }
    fn bytes(&mut self, field: u32, b: &[u8]) {
        self.tag(field, 2);
        self.varint(b.len() as u64);
        self.0.extend_from_slice(b);
    }
    fn str(&mut self, field: u32, s: &str) {
        self.bytes(field, s.as_bytes());
    }
    fn msg(&mut self, field: u32, m: Buf) {
        self.bytes(field, &m.0);
    }
}

pub enum Attr {
    Int(i64),
    Ints(Vec<i64>),
}

pub struct Node {
    pub op: &'static str,
    pub inputs: Vec<String>,
    pub output: String,
    pub attrs: Vec<(&'static str, Attr)>,
}

pub struct Initializer {
    pub name: String,
    pub dims: Vec<i64>,
    pub data: Vec<f32>,
}

/// Eingang/Ausgang: Name und Form; `None` = dynamische Dimension.
pub struct Io {
    pub name: String,
    pub dims: Vec<Option<i64>>,
}

pub struct Graph {
    pub nodes: Vec<Node>,
    pub initializers: Vec<Initializer>,
    pub inputs: Vec<Io>,
    pub outputs: Vec<Io>,
}

fn value_info(io: &Io) -> Buf {
    let mut shape = Buf::default();
    for (i, d) in io.dims.iter().enumerate() {
        let mut dim = Buf::default();
        match d {
            Some(v) => dim.int(1, *v),
            None => dim.str(2, &format!("d{i}")),
        }
        shape.msg(1, dim);
    }
    let mut tensor = Buf::default();
    tensor.int(1, 1); // FLOAT
    tensor.msg(2, shape);
    let mut ty = Buf::default();
    ty.msg(1, tensor);
    let mut vi = Buf::default();
    vi.str(1, &io.name);
    vi.msg(2, ty);
    vi
}

/// Serialisiert das Modell (Opset 17).
pub fn encode(g: &Graph) -> Vec<u8> {
    let mut graph = Buf::default();
    for (i, n) in g.nodes.iter().enumerate() {
        let mut node = Buf::default();
        for inp in &n.inputs {
            node.str(1, inp);
        }
        node.str(2, &n.output);
        node.str(3, &format!("n{i}"));
        node.str(4, n.op);
        for (name, a) in &n.attrs {
            let mut attr = Buf::default();
            attr.str(1, name);
            match a {
                Attr::Int(v) => {
                    attr.int(3, *v);
                    attr.int(20, 2);
                }
                Attr::Ints(v) => {
                    for x in v {
                        attr.int(8, *x);
                    }
                    attr.int(20, 7);
                }
            }
            node.msg(5, attr);
        }
        graph.msg(1, node);
    }
    graph.str(2, "lumenkorn");
    for t in &g.initializers {
        let mut tensor = Buf::default();
        for d in &t.dims {
            tensor.int(1, *d);
        }
        tensor.int(2, 1); // FLOAT
        tensor.str(8, &t.name);
        let raw: Vec<u8> = t.data.iter().flat_map(|v| v.to_le_bytes()).collect();
        tensor.bytes(9, &raw);
        graph.msg(5, tensor);
    }
    for io in &g.inputs {
        graph.msg(11, value_info(io));
    }
    for io in &g.outputs {
        graph.msg(12, value_info(io));
    }

    let mut model = Buf::default();
    model.int(1, 8); // ir_version
    model.str(2, "lumenkorn");
    let mut opset = Buf::default();
    opset.str(1, "");
    opset.int(2, 17);
    model.msg(8, opset);
    model.msg(7, graph);
    model.0
}
