// Entwicklungspipeline auf der GPU – Stufe für Stufe dieselben Formeln wie
// pipeline.rs (CPU). Jede Änderung dort muss hier nachgezogen werden; der Test
// `gpu_matches_cpu` vergleicht beide Wege Pixel für Pixel.
//
// Die festen Zahlen der Formeln (RAW_WHITE, HSL_CENTERS, SHARPEN_GAIN …) sind
// hier nicht definiert: `gpu::shader_source` stellt sie aus pipeline.rs und
// color.rs als WGSL-Konstanten voran.
//
// Bilder liegen als array<vec4<f32>> (RGB + ungenutzt), Helligkeit, Masken und
// Karten als array<f32>. Jeder Durchlauf bekommt eigene Parameter über einen
// dynamischen Offset in den Uniform-Puffer.

struct U {
    w: u32, h: u32, mw: u32, mh: u32,
    dw: u32, dh: u32, radius: u32, flags: u32,
    wb_r: f32, wb_g: f32, wb_b: f32, gain: f32,
    denoise: f32, fx: f32, fy: f32, amount: f32,
    black: f32, white: f32, shadows: f32, highlights: f32,
    contrast: f32, clarity: f32, sat: f32, vib: f32,
    out_w: u32, out_h: u32, rot: u32, flip: u32,
    ow: u32, oh: u32, crop_x: u32, crop_y: u32,
    st_s: f32, st_c: f32, st_k: f32, dehaze: f32,
    haze: f32, haze_lift: f32, _p2: f32, _p3: f32,
    // HSL je Farbbereich (−1…1): [0..2) Farbton, [2..4) Sättigung, [4..6) Luminanz
    hsl: array<vec4<f32>, 6>,
    // Color Grading (pipeline.rs: GradeVals): Tiefen, Mitten, Lichter, Global
    // je Tönung (xyz) und Helligkeit (w); grade_pw = Grenze, Übergangsbreite
    grade: array<vec4<f32>, 4>,
    grade_pw: vec4<f32>,
};

const F_RAW: u32 = 1u;
const F_DENOISE: u32 = 2u;
const F_BASE: u32 = 4u;
const F_DETAIL: u32 = 8u;
const F_PACK: u32 = 16u;
const F_STRAIGHTEN: u32 = 32u;

@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var<storage, read> ina: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> inb: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> outa: array<vec4<f32>>;
@group(0) @binding(4) var<storage, read> fa: array<f32>;
@group(0) @binding(5) var<storage, read> fb: array<f32>;
@group(0) @binding(6) var<storage, read> fc: array<f32>;
@group(0) @binding(7) var<storage, read_write> fo: array<f32>;
@group(0) @binding(8) var<storage, read_write> packed: array<u32>;

// --- Hilfsfunktionen (color.rs, pipeline.rs) ----------------------------------

fn luma(c: vec3<f32>) -> f32 {
    return 0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z;
}

fn srgb_to_linear(v: f32) -> f32 {
    if (v <= 0.04045) {
        return v / 12.92;
    }
    return pow((v + 0.055) / 1.055, 2.4);
}

fn linear_to_srgb(v: f32) -> f32 {
    if (v <= 0.0) {
        return 0.0;
    }
    if (v >= 1.0) {
        return 1.0;
    }
    if (v <= 0.0031308) {
        return v * 12.92;
    }
    return 1.055 * pow(v, 1.0 / 2.4) - 0.055;
}

fn enc3(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(linear_to_srgb(c.x), linear_to_srgb(c.y), linear_to_srgb(c.z));
}

fn sstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

fn fit_gamut(c: vec3<f32>, y_in: f32) -> vec3<f32> {
    let y = clamp(y_in, 0.0, 1.0);
    let mx = max(c.x, max(c.y, c.z));
    let mn = min(c.x, min(c.y, c.z));
    var k = 1.0;
    if (mx > 1.0 && mx > y) {
        k = min(k, (1.0 - y) / (mx - y));
    }
    if (mn < 0.0 && mn < y) {
        k = min(k, y / (y - mn));
    }
    var o = c;
    if (k < 1.0) {
        o = vec3<f32>(y) + (c - vec3<f32>(y)) * k;
    }
    return clamp(o, vec3<f32>(0.0), vec3<f32>(1.0));
}

fn apply_luma(c: vec3<f32>, y_old: f32, y_new: f32) -> vec3<f32> {
    var o: vec3<f32>;
    if (y_old > 1e-4) {
        o = c * (y_new / y_old);
    } else {
        o = c + vec3<f32>(y_new);
    }
    return fit_gamut(o, y_new);
}

fn s_curve(v: f32, amount: f32) -> f32 {
    if (amount == 0.0) {
        return v;
    }
    var tgt: f32;
    if (amount > 0.0) {
        tgt = v * v * (3.0 - 2.0 * v);
    } else {
        tgt = 0.5 - sin(asin(1.0 - 2.0 * v) / 3.0);
    }
    return v + (tgt - v) * min(abs(amount), 1.0) * CONTRAST_MIX;
}

fn base_t(v: f32) -> f32 {
    let x = v * RAW_BASE_GAIN;
    let w2 = RAW_WHITE * RAW_WHITE;
    return min(x * (1.0 + x / w2) / (1.0 + x), 1.0);
}

fn base_curve(c: vec3<f32>) -> vec3<f32> {
    let mx = max(c.x, max(c.y, c.z));
    if (mx <= 1e-6) {
        return vec3<f32>(0.0);
    }
    let tm = base_t(mx);
    var o = c * (tm / mx);
    let desat = sstep(0.82, 1.0, tm);
    if (desat > 0.0) {
        let y = max(luma(o), tm * 0.9);
        o = min(o + (vec3<f32>(y) - o) * desat, vec3<f32>(1.0));
    }
    return o;
}

fn soft_clip(v: f32) -> f32 {
    let k = SOFT_CLIP_KNEE;
    if (v <= k) {
        return max(v, 0.0);
    }
    return k + (1.0 - k) * (1.0 - exp(-(v - k) / (1.0 - k)));
}

// Bilineare Abtastung einer Helligkeitskarte (local.rs: LumaMap::sample).
fn sample_map(which: u32, mw: u32, mh: u32, x: u32, y: u32) -> f32 {
    let sx = f32(mw) / f32(u.w);
    let sy = f32(mh) / f32(u.h);
    let fx = clamp((f32(x) + 0.5) * sx - 0.5, 0.0, f32(mw - 1u));
    let fy = clamp((f32(y) + 0.5) * sy - 0.5, 0.0, f32(mh - 1u));
    let x0 = u32(fx);
    let y0 = u32(fy);
    let x1 = min(x0 + 1u, mw - 1u);
    let y1 = min(y0 + 1u, mh - 1u);
    let tx = fx - f32(x0);
    let ty = fy - f32(y0);
    var a: f32; var b: f32; var c: f32; var d: f32;
    if (which == 0u) {
        a = fb[y0 * mw + x0]; b = fb[y0 * mw + x1]; c = fb[y1 * mw + x0]; d = fb[y1 * mw + x1];
    } else {
        a = fc[y0 * mw + x0]; b = fc[y0 * mw + x1]; c = fc[y1 * mw + x0]; d = fc[y1 * mw + x1];
    }
    let top = a + (b - a) * tx;
    let bot = c + (d - c) * tx;
    return top + (bot - top) * ty;
}

// --- Durchläufe ---------------------------------------------------------------

// Entrauschen einmischen, Weißabgleich, Belichtung, Basiskurve, sRGB-Kodierung.
// ina = Quelle (linear), inb = entrauschte Fassung, outa = Arbeitsbild
@compute @workgroup_size(16, 16)
fn prep(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= u.w || id.y >= u.h) { return; }
    let i = id.y * u.w + id.x;
    var c = ina[i].xyz;
    if ((u.flags & F_DENOISE) != 0u) {
        c = c + (inb[i].xyz - c) * u.denoise;
    }
    c = c * vec3<f32>(u.wb_r, u.wb_g, u.wb_b) * u.gain;
    if ((u.flags & F_RAW) != 0u) {
        c = base_curve(c);
    } else {
        c = min(c, vec3<f32>(1.0));
    }
    outa[i] = vec4<f32>(enc3(c), 1.0);
}

// ina = Bild, fo = Helligkeit
@compute @workgroup_size(16, 16)
fn luma_pass(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= u.w || id.y >= u.h) { return; }
    let i = id.y * u.w + id.x;
    fo[i] = luma(ina[i].xyz);
}

// Flächenmittel der Helligkeit (fa, w×h) auf die Kartengröße (fo, mw×mh).
@compute @workgroup_size(16, 16)
fn map_down(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= u.mw || id.y >= u.mh) { return; }
    let x0 = u32(f32(id.x) * u.fx);
    let x1 = clamp(u32(f32(id.x + 1u) * u.fx), x0 + 1u, u.w);
    let y0 = u32(f32(id.y) * u.fy);
    let y1 = clamp(u32(f32(id.y + 1u) * u.fy), y0 + 1u, u.h);
    var s = 0.0;
    for (var y = y0; y < y1; y++) {
        var row = 0.0;
        for (var x = x0; x < x1; x++) {
            row += fa[y * u.w + x];
        }
        s += row;
    }
    fo[id.y * u.mw + id.x] = s / f32((x1 - x0) * (y1 - y0));
}

// Kastenfilter waagerecht: eine Zeile pro Thread (fa → fo, Größe mw×mh)
@compute @workgroup_size(64)
fn blur_h(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= u.mh) { return; }
    let row = id.x * u.mw;
    let r = i32(u.radius);
    let n = f32(2 * r + 1);
    let last = i32(u.mw) - 1;
    var acc = 0.0;
    for (var k = -r; k <= r; k++) {
        acc += fa[row + u32(clamp(k, 0, last))];
    }
    for (var x = 0; x <= last; x++) {
        fo[row + u32(x)] = acc / n;
        acc += fa[row + u32(clamp(x + r + 1, 0, last))] - fa[row + u32(clamp(x - r, 0, last))];
    }
}

// Kastenfilter senkrecht: eine Spalte pro Thread
@compute @workgroup_size(64)
fn blur_v(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= u.mw) { return; }
    let col = id.x;
    let r = i32(u.radius);
    let n = f32(2 * r + 1);
    let last = i32(u.mh) - 1;
    var acc = 0.0;
    for (var k = -r; k <= r; k++) {
        acc += fa[u32(clamp(k, 0, last)) * u.mw + col];
    }
    for (var y = 0; y <= last; y++) {
        fo[u32(y) * u.mw + col] = acc / n;
        acc += fa[u32(clamp(y + r + 1, 0, last)) * u.mw + col]
             - fa[u32(clamp(y - r, 0, last)) * u.mw + col];
    }
}

// Color Grading (pipeline.rs: grade_px), in place
@compute @workgroup_size(16, 16)
fn grade(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= u.w || id.y >= u.h) { return; }
    let i = id.y * u.w + id.x;
    let c = outa[i].xyz;
    let y = luma(c);
    let p = u.grade_pw.x;
    let wd = u.grade_pw.y;
    let ws = 1.0 - sstep(p - wd, p, y);
    let wh = sstep(p, p + wd, y);
    let wm = max(1.0 - ws - wh, 0.0);
    let ws4 = array<f32, 4>(ws, wm, wh, 1.0);
    var o = c;
    var yn = y;
    for (var k = 0u; k < 4u; k++) {
        let v = u.grade[k];
        o += ws4[k] * (v.xyz + vec3<f32>(v.w));
        yn += ws4[k] * v.w;
    }
    outa[i] = vec4<f32>(fit_gamut(o, yn), 1.0);
}

// Schwarzweiß (pipeline.rs: monochrome), in place
@compute @workgroup_size(16, 16)
fn mono(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= u.w || id.y >= u.h) { return; }
    let i = id.y * u.w + id.x;
    let y = luma(outa[i].xyz);
    outa[i] = vec4<f32>(y, y, y, outa[i].w);
}

// Dunkelkanal (kleinster Farbanteil): ina = Bild, fo = Wert je Pixel
@compute @workgroup_size(16, 16)
fn dark_pass(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= u.w || id.y >= u.h) { return; }
    let i = id.y * u.w + id.x;
    let c = ina[i].xyz;
    fo[i] = min(c.x, min(c.y, c.z));
}

// Dehaze (pipeline.rs: dehaze/dehaze_px), in place: outa = Bild,
// fa = Dunkelkanal je Pixel, fb = weichgezeichneter Dunkelkanal (mw×mh)
@compute @workgroup_size(16, 16)
fn dehaze_pass(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= u.w || id.y >= u.h) { return; }
    let i = id.y * u.w + id.x;
    let c = outa[i].xyz;
    let a = u.haze;
    if (u.dehaze < 0.0) {
        outa[i] = vec4<f32>(c + (vec3<f32>(a) - c) * (-u.dehaze * HAZE_ADD), 1.0);
        return;
    }
    let d = min(fa[i], sample_map(0u, u.mw, u.mh, id.x, id.y));
    let t = max(1.0 - u.dehaze * DEHAZE_GAIN * d / a, DEHAZE_MIN_T);
    let y = luma(c);
    var yn = (y - a) / t + a;
    let v = clamp(yn, 0.0, 1.0);
    yn = yn + u.haze_lift * 4.0 * v * (1.0 - v);
    let k = pow(t, -DEHAZE_CHROMA);
    outa[i] = vec4<f32>(fit_gamut(vec3<f32>(yn) + (c - vec3<f32>(y)) * k, yn), 1.0);
}

// Tonwerte: outa = Bild (in place), fa = Helligkeit, fb = grobe Karte (mw×mh),
// fc = feine Karte (dw×dh)
@compute @workgroup_size(16, 16)
fn tone(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= u.w || id.y >= u.h) { return; }
    let i = id.y * u.w + id.x;
    let y0 = fa[i];
    var v = (y0 - u.black) / (u.white - u.black);
    if ((u.flags & F_BASE) != 0u) {
        let b = sample_map(0u, u.mw, u.mh, id.x, id.y);
        if (u.shadows != 0.0) {
            let s = 1.0 - sstep(0.0, 0.55, b);
            let ws = s * s;
            v *= exp(u.shadows * SHADOW_HIGHLIGHT_GAIN * ws * max(1.0 - v, 0.0));
        }
        if (u.highlights != 0.0) {
            let s = sstep(0.4, 1.0, b);
            let wh = s * s;
            v = 1.0 - (1.0 - v) * exp(-u.highlights * SHADOW_HIGHLIGHT_GAIN * wh * max(v, 0.0));
        }
    }
    v = s_curve(clamp(v, 0.0, 1.0), u.contrast);
    if ((u.flags & F_DETAIL) != 0u) {
        let d = sample_map(1u, u.dw, u.dh, id.x, id.y);
        let mid = 4.0 * v * (1.0 - v);
        v += u.clarity * CLARITY_GAIN * (y0 - d) * mid;
    }
    outa[i] = vec4<f32>(apply_luma(outa[i].xyz, y0, clamp(v, 0.0, 1.0)), 1.0);
}

// Dynamik und Sättigung, in place
@compute @workgroup_size(16, 16)
fn color(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= u.w || id.y >= u.h) { return; }
    let i = id.y * u.w + id.x;
    let p = outa[i].xyz;
    let y = luma(p);
    let chroma = max(p.x, max(p.y, p.z)) - min(p.x, min(p.y, p.z));
    var skin = 1.0;
    if (p.x > p.y && p.y > p.z) {
        skin = 0.5;
    }
    var vk = 1.0;
    if (u.vib > 0.0) {
        vk = skin;
    }
    let f = 1.0 + u.sat + u.vib * (1.0 - chroma) * (1.0 - chroma) * vk;
    outa[i] = vec4<f32>(fit_gamut(vec3<f32>(y) + (p - vec3<f32>(y)) * f, y), 1.0);
}

// --- HSL (color.rs: oklab, oklab_to_linear, hsl_weights, hsl_pixel) ---------

fn oklab(c: vec3<f32>) -> vec3<f32> {
    let r = max(c.x, 0.0);
    let g = max(c.y, 0.0);
    let b = max(c.z, 0.0);
    let l = pow(0.41222146 * r + 0.53633255 * g + 0.051445995 * b, 1.0 / 3.0);
    let m = pow(0.2119035 * r + 0.6806995 * g + 0.10739696 * b, 1.0 / 3.0);
    let s = pow(0.08830246 * r + 0.28171885 * g + 0.6299787 * b, 1.0 / 3.0);
    return vec3<f32>(
        0.21045426 * l + 0.7936178 * m - 0.004072047 * s,
        1.9779985 * l - 2.4285922 * m + 0.4505937 * s,
        0.025904037 * l + 0.78277177 * m - 0.80867577 * s,
    );
}

fn oklab_to_linear(c: vec3<f32>) -> vec3<f32> {
    let l0 = c.x + 0.39633778 * c.y + 0.21580376 * c.z;
    let m0 = c.x - 0.105561346 * c.y - 0.06385417 * c.z;
    let s0 = c.x - 0.08948418 * c.y - 1.2914855 * c.z;
    let l = l0 * l0 * l0;
    let m = m0 * m0 * m0;
    let s = s0 * s0 * s0;
    return vec3<f32>(
        4.0767417 * l - 3.3077116 * m + 0.23096994 * s,
        -1.268438 * l + 2.6097574 * m - 0.34131938 * s,
        -0.0041960863 * l - 0.7034186 * m + 1.7076147 * s,
    );
}

fn hsl_band(k: u32, i: u32) -> f32 {
    // k: 0 Farbton, 1 Sättigung, 2 Luminanz
    return u.hsl[k * 2u + i / 4u][i % 4u];
}

@compute @workgroup_size(16, 16)
fn hsl(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= u.w || id.y >= u.h) { return; }
    let idx = id.y * u.w + id.x;
    let p = clamp(outa[idx].xyz, vec3<f32>(0.0), vec3<f32>(1.0));
    let lab = oklab(vec3<f32>(srgb_to_linear(p.x), srgb_to_linear(p.y), srgb_to_linear(p.z)));
    let c = sqrt(lab.y * lab.y + lab.z * lab.z);
    if (c < 1e-5) { return; }
    let hue = degrees(atan2(lab.z, lab.y));
    var h = hue - floor(hue / 360.0) * 360.0;
    var dh = 0.0;
    var ds = 0.0;
    var dl = 0.0;
    for (var i = 0u; i < 8u; i = i + 1u) {
        let a = HSL_CENTERS[i];
        let j = (i + 1u) % 8u;
        var b = HSL_CENTERS[j];
        if (b <= a) { b = b + 360.0; }
        var x = h;
        if (x < a) { x = x + 360.0; }
        if (x >= a && x < b) {
            let t = (x - a) / (b - a);
            dh = (1.0 - t) * hsl_band(0u, i) + t * hsl_band(0u, j);
            ds = (1.0 - t) * hsl_band(1u, i) + t * hsl_band(1u, j);
            dl = (1.0 - t) * hsl_band(2u, i) + t * hsl_band(2u, j);
            break;
        }
    }
    let g = sstep(0.0, 0.04, c);
    let h2 = radians(hue + dh * 30.0 * g);
    let c2 = c * max(1.0 + ds, 0.0);
    let l2 = lab.x * (1.0 + dl * 0.5 * g);
    let o = clamp(oklab_to_linear(vec3<f32>(l2, c2 * cos(h2), c2 * sin(h2))), vec3<f32>(0.0), vec3<f32>(1.0));
    outa[idx] = vec4<f32>(enc3(o), 1.0);
}

// Belichtung/Weißabgleich einer Maskenebene (auf sRGB-kodiertem Bild), in place
@compute @workgroup_size(16, 16)
fn local_wb(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= u.w || id.y >= u.h) { return; }
    let i = id.y * u.w + id.x;
    let p = outa[i].xyz;
    let wb = vec3<f32>(u.wb_r, u.wb_g, u.wb_b);
    let lin = vec3<f32>(
        soft_clip(srgb_to_linear(p.x) * wb.x * u.gain),
        soft_clip(srgb_to_linear(p.y) * wb.y * u.gain),
        soft_clip(srgb_to_linear(p.z) * wb.z * u.gain),
    );
    outa[i] = vec4<f32>(fit_gamut(enc3(lin), linear_to_srgb(luma(lin))), 1.0);
}

// Maskenebene einblenden: outa = Bild, ina = Ebene, fa = Deckkraft
@compute @workgroup_size(16, 16)
fn blend(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= u.w || id.y >= u.h) { return; }
    let i = id.y * u.w + id.x;
    let a = fa[i];
    if (a > 0.0) {
        let o = outa[i].xyz;
        outa[i] = vec4<f32>(o + (ina[i].xyz - o) * a, 1.0);
    }
}

// Unscharf maskieren: ina = Bild, fa = Helligkeit, outa = Ergebnis
@compute @workgroup_size(16, 16)
fn sharpen(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= u.w || id.y >= u.h) { return; }
    let x = i32(id.x);
    let y = i32(id.y);
    let mx = i32(u.w) - 1;
    let my = i32(u.h) - 1;
    let xm = clamp(x - 1, 0, mx);
    let xp = clamp(x + 1, 0, mx);
    let ym = clamp(y - 1, 0, my);
    let yp = clamp(y + 1, 0, my);
    let w = u32(u.w);
    let c = fa[u32(y) * w + u32(x)];
    let blur = (4.0 * c
        + 2.0 * (fa[u32(y) * w + u32(xm)] + fa[u32(y) * w + u32(xp)]
               + fa[u32(ym) * w + u32(x)] + fa[u32(yp) * w + u32(x)])
        + fa[u32(ym) * w + u32(xm)] + fa[u32(ym) * w + u32(xp)]
        + fa[u32(yp) * w + u32(xm)] + fa[u32(yp) * w + u32(xp)]) / 16.0;
    var d = c - blur;
    if (abs(d) < SHARPEN_THRESHOLD) {
        d = 0.0;
    }
    let delta = u.amount * SHARPEN_GAIN * d;
    let i = id.y * u.w + id.x;
    outa[i] = vec4<f32>(fit_gamut(ina[i].xyz + vec3<f32>(delta), c + delta), 1.0);
}

// Maske rot überlagern: outa = Bild, fa = Deckkraft
@compute @workgroup_size(16, 16)
fn overlay(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= u.w || id.y >= u.h) { return; }
    let i = id.y * u.w + id.x;
    let k = fa[i] * OVERLAY_OPACITY;
    let p = outa[i].xyz;
    outa[i] = vec4<f32>(p + (OVERLAY_COLOR - p) * k, 1.0);
}

// Pixel des gedrehten und gespiegelten Bildes (Größe ow×oh) aus dem
// Arbeitsbild (w×h) holen – wie rotate_quarters und danach flip_h.
fn oriented(xo_in: u32, yo: u32) -> vec3<f32> {
    var xo = xo_in;
    if (u.flip != 0u) {
        xo = u.ow - 1u - xo;
    }
    var sx = xo;
    var sy = yo;
    switch (u.rot) {
        case 1u: { sx = yo; sy = u.h - 1u - xo; }
        case 2u: { sx = u.w - 1u - xo; sy = u.h - 1u - yo; }
        case 3u: { sx = u.w - 1u - yo; sy = xo; }
        default: {}
    }
    return ina[sy * u.w + sx].xyz;
}

// Geometrie: drehen, spiegeln, gerade richten, zuschneiden. ina = Bild,
// Ausgabe nach outa (Float) oder packed (RGBA8).
@compute @workgroup_size(16, 16)
fn geom(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= u.out_w || id.y >= u.out_h) { return; }
    let px = id.x + u.crop_x;
    let py = id.y + u.crop_y;
    var c: vec3<f32>;
    if ((u.flags & F_STRAIGHTEN) != 0u) {
        let cx = f32(u.ow) / 2.0;
        let cy = f32(u.oh) / 2.0;
        let dx = (f32(px) + 0.5 - cx) / u.st_k;
        let dy = (f32(py) + 0.5 - cy) / u.st_k;
        let sx = clamp(u.st_c * dx + u.st_s * dy + cx - 0.5, 0.0, f32(u.ow - 1u));
        let sy = clamp(-u.st_s * dx + u.st_c * dy + cy - 0.5, 0.0, f32(u.oh - 1u));
        let x0 = u32(sx);
        let y0 = u32(sy);
        let x1 = min(x0 + 1u, u.ow - 1u);
        let y1 = min(y0 + 1u, u.oh - 1u);
        let tx = sx - f32(x0);
        let ty = sy - f32(y0);
        let a = oriented(x0, y0);
        let b = oriented(x1, y0);
        let cc = oriented(x0, y1);
        let d = oriented(x1, y1);
        let top = a + (b - a) * tx;
        let bot = cc + (d - cc) * tx;
        c = top + (bot - top) * ty;
    } else {
        c = oriented(px, py);
    }
    let o = id.y * u.out_w + id.x;
    if ((u.flags & F_PACK) != 0u) {
        let q = vec3<u32>(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)) * 255.0 + vec3<f32>(0.5));
        packed[o] = q.x | (q.y << 8u) | (q.z << 16u) | (255u << 24u);
    } else {
        outa[o] = vec4<f32>(c, 1.0);
    }
}
