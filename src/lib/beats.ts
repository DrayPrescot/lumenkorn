// Takterkennung für die Diashow-Musik.
//
// 1. Einsatzstärke (Onsets): Spektrum in kurzen Fenstern, Zunahme der
//    (logarithmierten) Energie von Fenster zu Fenster („spectral flux“).
// 2. Tempo: Autokorrelation der Einsatzstärke, gewichtet um 120 BPM.
// 3. Schläge: dynamische Programmierung (nach D. Ellis, „Beat Tracking by
//    Dynamic Programming“, 2007) – Schläge fallen auf starke Einsätze und
//    halten dabei das Tempo.
// 4. Taktanfang: Von den vier möglichen Lagen eines 4/4-Takts die mit den
//    stärksten Schlägen.

export interface BeatInfo {
  /** Länge des Stücks (s) */
  duration: number;
  /** Tempo (Schläge pro Minute), 0 wenn keins erkannt */
  bpm: number;
  /** Zeitpunkte der Schläge (s) */
  beats: number[];
  /** Stärke je Schlag (0…1) */
  strength: number[];
  /** Index des ersten Taktanfangs (0…3): jeder vierte Schlag ab hier */
  bar: number;
}

/** Arbeitsabtastrate: reicht für Einsätze, spart Rechenzeit */
const TARGET_RATE = 11025;
const WIN = 1024;
const HOP = 256;
const MIN_BPM = 60;
const MAX_BPM = 200;
/** Bevorzugtes Tempo und Breite der Gewichtung (Oktaven) */
const PRIOR_BPM = 120;
const PRIOR_OCT = 0.5;
/** Wie streng das Tempo gehalten wird (Ellis: „tightness“) */
const TIGHTNESS = 100;
/** Effektivwert, auf den das Signal vor der Analyse gebracht wird */
const RMS = 0.1;

/** Stereo/Mehrkanal zu Mono */
export function downmix(channels: Float32Array[]): Float32Array {
  if (channels.length === 1) return channels[0];
  const n = channels[0].length;
  const out = new Float32Array(n);
  for (const c of channels) for (let i = 0; i < n; i++) out[i] += c[i];
  const k = 1 / channels.length;
  for (let i = 0; i < n; i++) out[i] *= k;
  return out;
}

/** Komplexe FFT (Radix 2, an Ort und Stelle). `re.length` muss eine Zweierpotenz sein. */
function fft(re: Float32Array, im: Float32Array) {
  const n = re.length;
  for (let i = 1, j = 0; i < n; i++) {
    let bit = n >> 1;
    for (; j & bit; bit >>= 1) j ^= bit;
    j ^= bit;
    if (i < j) {
      [re[i], re[j]] = [re[j], re[i]];
      [im[i], im[j]] = [im[j], im[i]];
    }
  }
  for (let len = 2; len <= n; len <<= 1) {
    const ang = (-2 * Math.PI) / len;
    const wr = Math.cos(ang);
    const wi = Math.sin(ang);
    for (let i = 0; i < n; i += len) {
      let cr = 1;
      let ci = 0;
      for (let k = 0; k < len / 2; k++) {
        const a = i + k;
        const b = a + len / 2;
        const tr = re[b] * cr - im[b] * ci;
        const ti = re[b] * ci + im[b] * cr;
        re[b] = re[a] - tr;
        im[b] = im[a] - ti;
        re[a] += tr;
        im[a] += ti;
        const nr = cr * wr - ci * wi;
        ci = cr * wi + ci * wr;
        cr = nr;
      }
    }
  }
}

/** Einsatzstärke je Fenster (normiert) und Fenster pro Sekunde */
export function onsetEnvelope(samples: Float32Array, rate: number): { env: Float32Array; fps: number } {
  // Auf ~11 kHz verkleinern (Mittel über Blöcke, zugleich Tiefpass)
  const f = Math.max(1, Math.floor(rate / TARGET_RATE));
  const r = rate / f;
  const m = Math.floor(samples.length / f);
  const x = new Float32Array(m);
  for (let i = 0; i < m; i++) {
    let s = 0;
    for (let k = 0; k < f; k++) s += samples[i * f + k];
    x[i] = s / f;
  }
  // Auf gleichen Effektivwert bringen: Die Kompression unten soll bei leisen
  // und lauten Aufnahmen gleich wirken
  let ms = 0;
  for (let i = 0; i < m; i++) ms += x[i] * x[i];
  const gain = RMS / (Math.sqrt(ms / Math.max(1, m)) || 1);
  for (let i = 0; i < m; i++) x[i] *= gain;
  const frames = Math.max(0, Math.floor((m - WIN) / HOP) + 1);
  const bins = WIN / 2;
  const hann = new Float32Array(WIN);
  for (let i = 0; i < WIN; i++) hann[i] = 0.5 - 0.5 * Math.cos((2 * Math.PI * i) / WIN);
  const re = new Float32Array(WIN);
  const im = new Float32Array(WIN);
  let prev = new Float32Array(bins);
  let cur = new Float32Array(bins);
  const env = new Float32Array(frames);
  for (let t = 0; t < frames; t++) {
    const off = t * HOP;
    for (let i = 0; i < WIN; i++) {
      re[i] = x[off + i] * hann[i];
      im[i] = 0;
    }
    fft(re, im);
    let flux = 0;
    for (let b = 1; b < bins; b++) {
      const mag = Math.sqrt(re[b] * re[b] + im[b] * im[b]);
      const v = Math.log1p(mag);
      cur[b] = v;
      if (t > 0 && v > prev[b]) flux += v - prev[b];
    }
    env[t] = flux;
    [prev, cur] = [cur, prev];
  }
  const fps = r / HOP;
  // Gleitenden Mittelwert (~0,4 s) abziehen, nur Zunahmen behalten, normieren
  const w = Math.max(1, Math.round(fps * 0.2));
  const out = new Float32Array(frames);
  let sum = 0;
  let count = 0;
  for (let i = 0; i < Math.min(frames, w); i++) ((sum += env[i]), count++);
  for (let t = 0; t < frames; t++) {
    if (t + w < frames) ((sum += env[t + w]), count++);
    if (t - w - 1 >= 0) ((sum -= env[t - w - 1]), count--);
    out[t] = Math.max(0, env[t] - sum / count);
  }
  let sq = 0;
  for (let t = 0; t < frames; t++) sq += out[t] * out[t];
  const sd = Math.sqrt(sq / Math.max(1, frames)) || 1;
  for (let t = 0; t < frames; t++) out[t] /= sd;
  return { env: out, fps };
}

/** Schlagabstand in Fenstern (Autokorrelation mit Tempo-Gewichtung), 0 wenn keiner */
export function estimatePeriod(env: Float32Array, fps: number): number {
  const minLag = Math.floor((fps * 60) / MAX_BPM);
  const maxLag = Math.min(env.length - 1, Math.ceil((fps * 60) / MIN_BPM));
  if (maxLag <= minLag) return 0;
  const ac = new Float32Array(maxLag + 2);
  for (let lag = minLag - 1; lag <= maxLag + 1; lag++) {
    let s = 0;
    for (let t = lag; t < env.length; t++) s += env[t] * env[t - lag];
    ac[lag] = s / (env.length - lag);
  }
  let best = 0;
  let bestScore = 0;
  for (let lag = minLag; lag <= maxLag; lag++) {
    const bpm = (fps * 60) / lag;
    const w = Math.exp(-0.5 * (Math.log2(bpm / PRIOR_BPM) / PRIOR_OCT) ** 2);
    const score = ac[lag] * w;
    if (score > bestScore) {
      bestScore = score;
      best = lag;
    }
  }
  if (!best || bestScore <= 0) return 0;
  // Feiner zwischen den Fenstern (Parabel durch die Nachbarn)
  const a = ac[best - 1];
  const b = ac[best];
  const c = ac[best + 1];
  const d = a - 2 * b + c;
  return d < 0 ? best + (0.5 * (a - c)) / d : best;
}

/** Schläge (Fensterindizes) per dynamischer Programmierung */
export function trackBeats(env: Float32Array, period: number): number[] {
  const n = env.length;
  if (!n || period <= 1) return [];
  const score = new Float32Array(n);
  const from = new Int32Array(n).fill(-1);
  const lo = Math.round(period / 2);
  const hi = Math.round(period * 2);
  for (let t = 0; t < n; t++) {
    let best = 0;
    let arg = -1;
    for (let tau = t - hi; tau <= t - lo; tau++) {
      if (tau < 0) continue;
      const pen = Math.log((t - tau) / period);
      const s = score[tau] - TIGHTNESS * pen * pen;
      if (arg < 0 || s > best) {
        best = s;
        arg = tau;
      }
    }
    score[t] = env[t] + (arg >= 0 ? Math.max(0, best) : 0);
    from[t] = arg >= 0 && best > 0 ? arg : -1;
  }
  // Ende: bester Wert im letzten Schlagabstand
  let end = n - 1;
  for (let t = Math.max(0, n - Math.round(period)); t < n; t++) if (score[t] > score[end]) end = t;
  const beats: number[] = [];
  for (let t = end; t >= 0; t = from[t]) beats.push(t);
  return beats.reverse();
}

/**
 * Oktavfehler beheben: Ist jeder zweite Schlag deutlich schwächer (z. B.
 * Achtel-Hi-Hats) und das Tempo hoch, war es doppelt so hoch wie gehört –
 * dann nur die starken Schläge behalten. Liegen umgekehrt bei langsamem Tempo
 * mitten zwischen den Schlägen ähnlich starke Einsätze (Kick und Snare im
 * Wechsel), war es halb so hoch.
 */
function fixOctave(
  frames: number[],
  period: number,
  fps: number,
  peak: (f: number) => number,
  mean: (xs: number[]) => number,
): { period: number; frames: number[] } {
  if (frames.length < 8) return { period, frames };
  const bpm = (fps * 60) / period;
  // Nur außerhalb des üblichen Bereichs: Betonungen (Snare auf 2 und 4) sind
  // sonst kein Oktavfehler
  if (bpm > 140) {
    const even = mean(frames.filter((_, i) => i % 2 === 0).map(peak));
    const odd = mean(frames.filter((_, i) => i % 2 === 1).map(peak));
    if (Math.min(even, odd) < 0.45 * Math.max(even, odd)) {
      const keep = even >= odd ? 0 : 1;
      return { period: period * 2, frames: frames.filter((_, i) => i % 2 === keep) };
    }
  }
  if (bpm < 90) {
    const mids = frames.slice(1).map((f, i) => (f + frames[i]) / 2);
    if (mean(mids.map(peak)) > 0.5 * mean(frames.map(peak))) {
      const out: number[] = [];
      frames.forEach((f, i) => {
        out.push(f);
        if (i < mids.length) out.push(Math.round(mids[i]));
      });
      return { period: period / 2, frames: out };
    }
  }
  return { period, frames };
}

/** Takt eines Musikstücks (Mono-Abtastwerte) */
export function detectBeats(samples: Float32Array, rate: number): BeatInfo {
  const duration = samples.length / rate;
  const { env, fps } = onsetEnvelope(samples, rate);
  let period = estimatePeriod(env, fps);
  if (!period) return { duration, bpm: 0, beats: [], strength: [], bar: 0 };
  let frames = trackBeats(env, period);
  // Stärke: größter Einsatz in der Nähe
  const peak = (f: number) => {
    let m = 0;
    for (let k = Math.max(0, Math.round(f) - 2); k <= Math.min(env.length - 1, Math.round(f) + 2); k++) m = Math.max(m, env[k]);
    return m;
  };
  const mean = (xs: number[]) => xs.reduce((s, x) => s + x, 0) / Math.max(1, xs.length);
  ({ period, frames } = fixOctave(frames, period, fps, peak, mean));
  // Zeit eines Fensters: seine Mitte
  const toTime = (f: number) => (f * HOP + WIN / 2) / (fps * HOP);
  const beats = frames.map(toTime);
  const raw = frames.map(peak);
  const sorted = [...raw].sort((a, b) => a - b);
  const ref = sorted[Math.floor(sorted.length * 0.95)] || 1;
  const strength = raw.map((v) => Math.min(1, v / ref));
  let bar = 0;
  let barScore = -1;
  for (let ph = 0; ph < 4; ph++) {
    let s = 0;
    for (let i = ph; i < strength.length; i += 4) s += strength[i];
    if (s > barScore) {
      barScore = s;
      bar = ph;
    }
  }
  return { duration, bpm: (fps * 60) / period, beats, strength, bar };
}

/** Ein Schlag der Musik, über das Stückende hinaus fortgesetzt */
interface Beat {
  t: number;
  strength: number;
  /** Taktanfang */
  down: boolean;
}

/**
 * Diashow: Legt die Bildwechsel auf Schläge. `d`: gewünschte Dauer je Dia.
 * Für jedes Dia wird der Schlag nahe der gewünschten Dauer gesucht, starke
 * Schläge und Taktanfänge bevorzugt, aber nicht vor `minSeconds`. Läuft die
 * Diashow länger als das Stück, beginnt die Musik von vorn – die Schläge auch.
 */
export function snapToBeats(d: number[], info: BeatInfo, minSeconds: number): { starts: number[]; total: number } {
  const need = d.reduce((x, y) => x + y, 0) * 1.6 + 10;
  const beats: Beat[] = [];
  for (let k = 0; info.duration > 0 && k * info.duration <= need; k++) {
    const off = k * info.duration;
    info.beats.forEach((t, i) => beats.push({ t: t + off, strength: info.strength[i] ?? 0.5, down: (((i - info.bar) % 4) + 4) % 4 === 0 }));
  }
  const starts: number[] = [];
  let prev = 0;
  let ideal = 0;
  let j = 0;
  for (let i = 0; i < d.length; i++) {
    starts.push(prev);
    // Ziel aus dem ungerundeten Zeitplan: Abweichungen summieren sich nicht auf
    ideal += d[i];
    const want = Math.min(prev + d[i] * 1.4, Math.max(prev + d[i] * 0.7, ideal));
    const lo = prev + Math.max(minSeconds, d[i] * 0.6);
    const hi = prev + d[i] * 1.5;
    while (j < beats.length && beats[j].t < lo) j++;
    let best = want;
    let bestCost = Infinity;
    for (let k = j; k < beats.length && beats[k].t <= hi; k++) {
      const b = beats[k];
      const cost = Math.abs(b.t - want) / d[i] - 0.3 * b.strength - (b.down ? 0.2 : 0);
      if (cost < bestCost) {
        bestCost = cost;
        best = b.t;
      }
    }
    prev = best;
  }
  return { starts, total: prev };
}
