// Key image pipeline: uploads → normalized PNG, key config → device-safe JPEG.
import sharp from 'sharp';

export const KEY_SIZE = 100;
export const UPLOAD_SIZE = 200;
export const MAX_UPLOAD_BYTES = 20 * 1024 * 1024;
const DEFAULT_BG = '#141618';
const DEFAULT_FG = '#eef0f2';

function badImage(message) {
  const err = new Error(message);
  err.code = 'BAD_IMAGE';
  return err;
}

/** Any image (first GIF frame) → 200×200 PNG, cover-cropped to the centre, alpha kept. */
export async function normalizeUpload(buf) {
  if (!Buffer.isBuffer(buf) || buf.length === 0) throw badImage('Prázdný soubor');
  if (buf.length > MAX_UPLOAD_BYTES) throw badImage('Obrázek je větší než 20 MB');
  try {
    return await sharp(buf, { pages: 1, limitInputPixels: 100_000_000 })
      .rotate()
      .resize(UPLOAD_SIZE, UPLOAD_SIZE, { fit: 'cover', position: 'centre' })
      .png()
      .toBuffer();
  } catch {
    throw badImage('Soubor není podporovaný obrázek');
  }
}

const escapeXml = (s) => s.replace(/[<>&"']/g, (c) => ({ '<': '&lt;', '>': '&gt;', '&': '&amp;', '"': '&quot;', "'": '&apos;' }[c]));

function labelSvg(label, color, withImage) {
  const text = label.length > 12 ? label.slice(0, 11) + '…' : label;
  const t = escapeXml(text);
  const font = 'font-family="Segoe UI, sans-serif" font-weight="700"';
  const body = withImage
    ? `<defs><linearGradient id="g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#000" stop-opacity="0"/><stop offset="1" stop-color="#000" stop-opacity="0.75"/></linearGradient></defs>
       <rect x="0" y="62" width="100" height="38" fill="url(#g)"/>
       <text x="50" y="93" text-anchor="middle" ${font} font-size="14" fill="${color}">${t}</text>`
    : `<text x="50" y="50" text-anchor="middle" dominant-baseline="central" ${font} font-size="18" fill="${color}">${t}</text>`;
  return Buffer.from(`<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">${body}</svg>`);
}

/** Renders one key to a 100×100 baseline JPEG, rotated 180° for the inverted panels. */
export async function renderKeyJpeg({ image = null, label = '', bg = DEFAULT_BG, labelColor = DEFAULT_FG } = {}) {
  const layers = [];
  if (image) {
    try {
      const img = await sharp(image).resize(KEY_SIZE, KEY_SIZE, { fit: 'cover' }).flatten({ background: bg }).png().toBuffer();
      layers.push({ input: img, top: 0, left: 0 });
    } catch {
      image = null; // unreadable image → bg + label
    }
  }
  if (label) layers.push({ input: labelSvg(label, labelColor, Boolean(image)), top: 0, left: 0 });

  const flat = await sharp({ create: { width: KEY_SIZE, height: KEY_SIZE, channels: 3, background: bg } })
    .composite(layers)
    .png()
    .toBuffer();
  const jpeg = await sharp(flat)
    .rotate(180)
    .jpeg({ quality: 90, progressive: false, mozjpeg: false, optimiseCoding: true })
    .toBuffer();
  return cleanBaselineJpeg(jpeg);
}

/** Keeps only SOI, APP0 (JFIF), DQT, SOF0, DHT and SOS…EOI. Port of clean_baseline_jpeg. */
// Standard JFIF 1.01 APP0, no thumbnail. The D6 decoder rejects JPEGs without it.
const JFIF_APP0 = Buffer.from([0xff, 0xe0, 0x00, 0x10, 0x4a, 0x46, 0x49, 0x46, 0x00, 0x01, 0x01, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00]);

export function cleanBaselineJpeg(data) {
  if (!data || data[0] !== 0xff || data[1] !== 0xd8) return data;
  const out = [Buffer.from([0xff, 0xd8])];
  if (!(data[2] === 0xff && data[3] === 0xe0)) out.push(JFIF_APP0);
  let i = 2;
  const n = data.length;
  while (i < n) {
    if (data[i] !== 0xff) { i++; continue; }
    while (i < n && data[i] === 0xff) i++;
    if (i >= n) break;
    const marker = data[i++];
    if (marker === 0xd9) { out.push(Buffer.from([0xff, 0xd9])); return Buffer.concat(out); }
    if ((marker >= 0xd0 && marker <= 0xd7) || marker === 0x01) continue;
    if (i + 2 > n) break;
    const len = (data[i] << 8) | data[i + 1];
    if (marker === 0xda) {
      out.push(Buffer.from([0xff, 0xda]), data.subarray(i));
      const all = Buffer.concat(out);
      return all[all.length - 2] === 0xff && all[all.length - 1] === 0xd9 ? all : Buffer.concat([all, Buffer.from([0xff, 0xd9])]);
    }
    if ([0xdb, 0xc0, 0xc4, 0xe0].includes(marker)) out.push(Buffer.from([0xff, marker]), data.subarray(i, i + len));
    i += len;
  }
  out.push(Buffer.from([0xff, 0xd9]));
  return Buffer.concat(out);
}

/** Second bytes of the header markers, in order, up to and including SOS (0xDA). */
export function jpegMarkers(buf) {
  const markers = [];
  if (buf[0] !== 0xff || buf[1] !== 0xd8) return markers;
  markers.push(0xd8);
  let i = 2;
  while (i + 3 < buf.length) {
    if (buf[i] !== 0xff) break;
    const m = buf[i + 1];
    markers.push(m);
    if (m === 0xda) break;
    i += 2 + ((buf[i + 2] << 8) | buf[i + 3]);
  }
  return markers;
}
