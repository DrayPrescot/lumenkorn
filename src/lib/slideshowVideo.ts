// Diashow als Video (MP4): Jedes Bild der Zeitleiste wird wie im Player auf
// eine Leinwand gezeichnet und mit WebCodecs kodiert (H.264 über die Encoder
// von Windows bzw. der Grafikkarte), die Musik als AAC. Mediabunny verpackt
// beides in MP4 und gibt die Daten stückweise mit ihrer Dateiposition aus.

import {
  AudioBufferSource,
  CanvasSource,
  Mp4OutputFormat,
  Output,
  QUALITY_HIGH,
  StreamTarget,
  canEncodeAudio,
  canEncodeVideo,
  type StreamTargetChunk,
} from "mediabunny";
import { imageUrl } from "./api";
import { fontFamily, type AlbumCandidate } from "./album";
import { clipGain, frameAt, slideAt, timeline, type AudioClip, type Frame, type SlideshowState } from "./slideshow";
import { drawMapLeg, drawOverview, mapData, type MapData, type MapStyle } from "./slideMap";
import { t } from "./i18n";

export type VideoSize = "720p" | "1080p" | "1440p";
export const VIDEO_SIZES: Record<VideoSize, [number, number]> = {
  "720p": [1280, 720],
  "1080p": [1920, 1080],
  "1440p": [2560, 1440],
};
export const VIDEO_FPS = 30;

/** Videocodecs zur Wahl (alle in MP4) */
export type VideoCodecKey = "avc" | "hevc" | "av1" | "vp9";
export const VIDEO_CODECS: Record<VideoCodecKey, string> = {
  avc: "H.264",
  hevc: "H.265 (HEVC)",
  av1: "AV1",
  vp9: "VP9",
};

/** Welche Codecs dieses System in der Größe kodieren kann */
export async function encodableCodecs(size: VideoSize): Promise<Set<VideoCodecKey>> {
  const [width, height] = VIDEO_SIZES[size];
  const out = new Set<VideoCodecKey>();
  for (const c of Object.keys(VIDEO_CODECS) as VideoCodecKey[]) {
    if (await canEncodeVideo(c, { width, height, bitrate: QUALITY_HIGH }).catch(() => false)) out.add(c);
  }
  return out;
}

/** Schrift wie in der Oberfläche */
const FONT = fontFamily("sans");

// --- Zeichnen ----------------------------------------------------------------------

type Source = CanvasImageSource & { width: number; height: number };

/** Ein Zeitpunkt der Diashow auf die Leinwand – dieselbe Gestaltung wie `SlideStage.svelte`. */
export function drawFrame(ctx: CanvasRenderingContext2D, frame: Frame, title: string, w: number, h: number, images: Map<number, Source>, map: MapData | null, style: MapStyle) {
  ctx.save();
  ctx.fillStyle = "#000";
  ctx.fillRect(0, 0, w, h);
  for (const l of frame.layers) {
    const img = images.get(l.id);
    ctx.save();
    ctx.globalAlpha = l.opacity;
    const x0 = l.shift * w;
    ctx.beginPath();
    ctx.rect(x0, 0, w, h);
    ctx.clip();
    ctx.translate(x0, 0);
    if (l.map) drawMapLeg(ctx, w, h, l.map.leg, l.map.p, map, style);
    else if (l.overview) drawOverview(ctx, w, h, l.overview.places, l.overview.p, map, style);
    else if (img) {
      if (l.contain) {
        // Unscharfer, dunklerer Hintergrund aus demselben Foto
        ctx.save();
        ctx.filter = `blur(${Math.round(h * 0.03)}px) brightness(0.55)`;
        drawCover(ctx, img, w, h, [0.5, 0.5], 1.15, 0, 0);
        ctx.restore();
        drawContain(ctx, img, w, h, l.scale);
      } else drawCover(ctx, img, w, h, l.pos, l.scale, l.tx, l.ty);
    }
    if (l.caption && l.captionOpacity > 0) {
      ctx.globalAlpha = l.opacity * l.captionOpacity;
      drawText(ctx, l.caption, {
        x: w * 0.05,
        bottom: h * 0.91,
        maxWidth: w * 0.7,
        size: h * 0.036,
        weight: 500,
        align: "left",
      });
    }
    ctx.restore();
  }
  if (title.trim() && frame.title > 0) {
    ctx.save();
    ctx.globalAlpha = frame.title;
    const g = ctx.createRadialGradient(w / 2, h / 2, 0, w / 2, h / 2, Math.max(w, h) * 0.5);
    g.addColorStop(0, "rgb(0 0 0 / 0.35)");
    g.addColorStop(1, "rgb(0 0 0 / 0)");
    ctx.fillStyle = g;
    ctx.fillRect(0, 0, w, h);
    drawText(ctx, title, { x: w / 2, middle: h / 2, maxWidth: w * 0.84, size: h * 0.08, weight: 600, align: "center" });
    ctx.restore();
  }
  if (frame.black > 0) {
    ctx.globalAlpha = frame.black;
    ctx.fillStyle = "#000";
    ctx.fillRect(0, 0, w, h);
  }
  ctx.restore();
}

/** object-fit: cover mit object-position, danach Ken Burns um die Mitte */
function drawCover(ctx: CanvasRenderingContext2D, img: Source, w: number, h: number, pos: [number, number], scale: number, tx: number, ty: number) {
  const s = Math.max(w / img.width, h / img.height);
  const dw = img.width * s;
  const dh = img.height * s;
  ctx.save();
  ctx.translate(w / 2 + tx * w, h / 2 + ty * h);
  ctx.scale(scale, scale);
  ctx.translate(-w / 2, -h / 2);
  ctx.drawImage(img, (w - dw) * pos[0], (h - dh) * pos[1], dw, dh);
  ctx.restore();
}

function drawContain(ctx: CanvasRenderingContext2D, img: Source, w: number, h: number, scale: number) {
  const s = Math.min(w / img.width, h / img.height) * scale;
  const dw = img.width * s;
  const dh = img.height * s;
  ctx.drawImage(img, (w - dw) / 2, (h - dh) / 2, dw, dh);
}

/** Text mit Schatten, umbrochen auf `maxWidth`; unten bündig (`bottom`) oder mittig (`middle`) */
function drawText(
  ctx: CanvasRenderingContext2D,
  text: string,
  o: { x: number; bottom?: number; middle?: number; maxWidth: number; size: number; weight: number; align: CanvasTextAlign },
) {
  ctx.save();
  ctx.font = `${o.weight} ${o.size}px ${FONT}`;
  ctx.textAlign = o.align;
  ctx.textBaseline = "alphabetic";
  ctx.fillStyle = "#fff";
  ctx.shadowColor = "rgb(0 0 0 / 0.75)";
  ctx.shadowBlur = o.size * 0.4;
  ctx.shadowOffsetY = o.size * 0.05;
  const lines: string[] = [];
  for (const para of text.split("\n")) {
    let line = "";
    for (const word of para.split(/\s+/)) {
      const next = line ? `${line} ${word}` : word;
      if (line && ctx.measureText(next).width > o.maxWidth) {
        lines.push(line);
        line = word;
      } else line = next;
    }
    lines.push(line);
  }
  const lh = o.size * 1.22;
  const top = o.bottom !== undefined ? o.bottom - lh * (lines.length - 1) : (o.middle ?? 0) - (lh * (lines.length - 1)) / 2 + o.size * 0.35;
  lines.forEach((l, i) => ctx.fillText(l, o.x, top + i * lh));
  ctx.restore();
}

// --- Bilder laden ------------------------------------------------------------------

/**
 * Foto in voller Auflösung laden (wie im Vollbild über ein Bild-Element) und
 * auf etwa die doppelte Videohöhe verkleinern – genug Reserve für den
 * Ken-Burns-Zoom, ohne riesige Bilder im Speicher. Ohne eigene Pixelgröße
 * (SVG der Browser-Vorschau) nach dem Seitenverhältnis.
 */
async function loadImage(id: number, version: number, aspect: number, maxEdge: number): Promise<ImageBitmap | null> {
  const load = async (variant: "full" | "preview") => {
    const img = new Image();
    img.crossOrigin = "anonymous";
    img.src = imageUrl(variant, id, version);
    await img.decode();
    return img;
  };
  try {
    // Volle Auflösung; gelingt sie nicht (Entwickeln fehlgeschlagen), die Vorschau
    const img = await load("full").catch(() => load("preview"));
    if (img.src.startsWith("data:image/svg")) {
      // SVG ohne eigene Größe (Browser-Vorschau): über eine Leinwand rastern,
      // createImageBitmap liefert dafür ein leeres Bild
      const c = document.createElement("canvas");
      c.width = Math.round(Math.min(maxEdge, 2048));
      c.height = Math.round(c.width / aspect);
      c.getContext("2d")!.drawImage(img, 0, 0, c.width, c.height);
      return await createImageBitmap(c);
    }
    const k = Math.min(1, maxEdge / Math.max(img.naturalWidth, img.naturalHeight));
    return await createImageBitmap(img, { resizeWidth: Math.round(img.naturalWidth * k), resizeHeight: Math.round(img.naturalHeight * k), resizeQuality: "high" });
  } catch {
    return null;
  }
}

// --- Ton ---------------------------------------------------------------------------

/**
 * Tonspur von `from` bis `to`: alle Musikstücke der Zeitleiste gemischt, mit
 * Lautstärke, Überblenden und demselben Ein- und Ausblenden wie im Player.
 */
function audioChunk(clips: AudioClip[], buffers: (AudioBuffer | null)[], rate: number, from: number, to: number, total: number, volume: number): AudioBuffer {
  const length = Math.max(1, Math.round((to - from) * rate));
  const out = new AudioBuffer({ length, numberOfChannels: 2, sampleRate: rate });
  const left = out.getChannelData(0);
  const right = out.getChannelData(1);
  for (const c of clips) {
    const buf = buffers[c.track];
    if (!buf || c.end <= from || c.start >= to) continue;
    const l = buf.getChannelData(0);
    const r = buf.numberOfChannels > 1 ? buf.getChannelData(1) : l;
    const sr = buf.sampleRate;
    const i0 = Math.max(0, Math.floor((c.start - from) * rate));
    const i1 = Math.min(length, Math.ceil((c.end - from) * rate));
    for (let i = i0; i < i1; i++) {
      const t = from + i / rate;
      const g = clipGain(c, t, total) * volume;
      if (g <= 0) continue;
      // Stelle im Lied (andere Abtastrate: nächster Wert genügt)
      const k = Math.floor((c.offset + t - c.start) * sr);
      if (k < 0 || k >= l.length) continue;
      left[i] += l[k] * g;
      right[i] += r[k] * g;
    }
  }
  return out;
}

// --- Export ------------------------------------------------------------------------

export interface VideoProgress {
  done: number;
  total: number;
}

/**
 * Rendert die Diashow als MP4. `write` bekommt die Daten mit Dateiposition,
 * `onProgress` die fertigen Bilder. Bricht mit einem Fehler ab, wenn `signal`
 * ausgelöst wird.
 */
export async function renderVideo(
  a: SlideshowState,
  byId: Map<number, AlbumCandidate>,
  version: (id: number) => number,
  music: (Blob | null)[],
  size: VideoSize,
  codec: VideoCodecKey,
  style: MapStyle,
  write: (chunk: StreamTargetChunk) => Promise<void>,
  onProgress: (p: VideoProgress) => void,
  signal: AbortSignal,
): Promise<void> {
  const [w, h] = VIDEO_SIZES[size];
  const tl = timeline(a, byId);
  const frames = Math.ceil(tl.total * VIDEO_FPS);
  if (!frames) throw t("Die Diashow ist leer.");

  if (!(await canEncodeVideo(codec, { width: w, height: h, bitrate: QUALITY_HIGH }).catch(() => false)))
    throw t("Dieses System kann {codec} in dieser Größe nicht kodieren.", { codec: VIDEO_CODECS[codec] });

  // Lieder dekodieren (gleiche Abtastrate für alle: die des AudioContext)
  const buffers: (AudioBuffer | null)[] = [];
  let rate = 48000;
  if (a.music && music.some(Boolean)) {
    const actx = new AudioContext();
    rate = actx.sampleRate;
    try {
      for (const m of music) buffers.push(m ? await actx.decodeAudioData(await m.arrayBuffer()).catch(() => null) : null);
    } finally {
      actx.close().catch(() => {});
    }
  }
  const hasAudio = buffers.some(Boolean) && tl.clips.length > 0;
  const audioCodec = hasAudio ? ((await canEncodeAudio("aac", { numberOfChannels: 2, sampleRate: rate })) ? "aac" : "opus") : null;

  const canvas = document.createElement("canvas");
  canvas.width = w;
  canvas.height = h;
  const ctx = canvas.getContext("2d", { alpha: false })!;
  await document.fonts.load(`600 20px ${FONT}`).catch(() => {});
  await document.fonts.load(`500 20px ${FONT}`).catch(() => {});
  const map = tl.entries.some((e) => e.kind === "map" || e.kind === "overview") ? await mapData().catch(() => null) : null;

  const output = new Output({
    format: new Mp4OutputFormat({ fastStart: false }),
    target: new StreamTarget(new WritableStream<StreamTargetChunk>({ write }), { chunked: true, chunkSize: 8 * 1024 * 1024 }),
  });
  const video = new CanvasSource(canvas, { codec, bitrate: QUALITY_HIGH, keyFrameInterval: 2 });
  output.addVideoTrack(video, { frameRate: VIDEO_FPS });
  const sound = hasAudio && audioCodec ? new AudioBufferSource({ codec: audioCodec, bitrate: QUALITY_HIGH }) : null;
  if (sound) output.addAudioTrack(sound);
  await output.start();

  const images = new Map<number, ImageBitmap>();
  const pending = new Map<number, Promise<ImageBitmap | null>>();
  const need = (id: number) => {
    if (images.has(id) || pending.has(id)) return;
    pending.set(
      id,
      loadImage(id, version(id), byId.get(id)?.aspect ?? 1.5, h * 2).then((b) => {
        pending.delete(id);
        if (b) images.set(id, b);
        return b;
      }),
    );
  };
  let audioUntil = 0;
  try {
    for (let f = 0; f < frames; f++) {
      if (signal.aborted) throw new DOMException(t("Abgebrochen"), "AbortError");
      const time = f / VIDEO_FPS;
      const frame = frameAt(a, tl, time, w / h, byId);
      // Sichtbare und die nächsten Fotos laden, ältere freigeben
      const cur = slideAt(tl.starts, time);
      const keep = new Set<number>();
      for (let i = Math.max(0, cur - 1); i < Math.min(tl.entries.length, cur + 3); i++) {
        const e = tl.entries[i];
        if (e.kind === "photo") keep.add(a.slides[e.slide].id);
        else if (e.kind === "title") keep.add(e.id);
      }
      for (const id of keep) need(id);
      for (const [id, b] of images) if (!keep.has(id)) (b.close(), images.delete(id));
      for (const l of frame.layers) if (!l.map && !l.overview && pending.has(l.id)) await pending.get(l.id);
      drawFrame(ctx, frame, a.title, w, h, images, map, style);
      // Ton vorausschicken, damit der Muxer beides verschränken kann
      if (sound && audioUntil < Math.min(tl.total, time + 1)) {
        const to = Math.min(tl.total, audioUntil + 2);
        await sound.add(audioChunk(tl.clips, buffers, rate, audioUntil, to, tl.total, a.music!.volume));
        audioUntil = to;
      }
      await video.add(time, 1 / VIDEO_FPS);
      if (f % 15 === 0) onProgress({ done: f, total: frames });
    }
    if (sound && audioUntil < tl.total) await sound.add(audioChunk(tl.clips, buffers, rate, audioUntil, tl.total, tl.total, a.music!.volume));
    await output.finalize();
    onProgress({ done: frames, total: frames });
  } catch (e) {
    await output.cancel().catch(() => {});
    throw e;
  } finally {
    for (const b of images.values()) b.close();
  }
}
