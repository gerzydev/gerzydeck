import { test } from 'node:test';
import assert from 'node:assert/strict';
import sharp from 'sharp';
import { renderKeyJpeg, normalizeUpload, jpegMarkers, cleanBaselineJpeg } from '../src/deck/render.js';

test('rendered key is a device-safe jpeg', async () => {
  const j = await renderKeyJpeg({ label: 'Fireball', bg: '#336699' });
  const m = jpegMarkers(j);
  assert.equal(m[0], 0xD8); assert.equal(m[1], 0xE0);
  assert.ok(m.includes(0xC0)); assert.ok(!m.includes(0xC2)); assert.ok(!m.includes(0xDD)); assert.ok(!m.includes(0xE1));
  for (let i = 0; i < j.length - 1; i++) if (j[i] === 0xFF) assert.ok(!(j[i + 1] >= 0xD0 && j[i + 1] <= 0xD7), 'RST marker');
  const meta = await sharp(j).metadata(); assert.equal(meta.width, 100); assert.equal(meta.height, 100);
  assert.ok(j.length < 0xFFFF);
});

test('image is rotated 180°', async () => {
  const top = await sharp({ create: { width: 200, height: 200, channels: 3, background: '#000' } })
    .composite([{ input: { create: { width: 200, height: 100, channels: 3, background: '#ff0000' } }, top: 0, left: 0 }]).png().toBuffer();
  const raw = await sharp(await renderKeyJpeg({ image: top })).raw().toBuffer();
  const px = (x, y) => raw[(y * 100 + x) * 3];
  assert.ok(px(50, 90) > 200); assert.ok(px(50, 10) < 50);
});

test('normalizeUpload: transparent png, gif, junk', async () => {
  const png = await sharp({ create: { width: 640, height: 300, channels: 4, background: { r: 0, g: 0, b: 0, alpha: 0 } } }).png().toBuffer();
  const n = await normalizeUpload(png);
  const meta = await sharp(n).metadata(); assert.equal(meta.format, 'png'); assert.equal(meta.width, 200); assert.equal(meta.height, 200);
  const gif = await sharp({ create: { width: 50, height: 50, channels: 3, background: '#0f0' } }).gif().toBuffer();
  assert.equal((await sharp(await normalizeUpload(gif)).metadata()).width, 200);
  await assert.rejects(normalizeUpload(Buffer.from('hello')), { code: 'BAD_IMAGE' });
});

test('missing image falls back to bg + label', async () => {
  assert.ok((await renderKeyJpeg({ image: null, label: 'X' })).length > 0);
});

test('cleanBaselineJpeg drops EXIF and DRI segments', () => {
  const seg = (m, payload) => Buffer.concat([Buffer.from([0xff, m, 0, payload.length + 2]), Buffer.from(payload)]);
  const src = Buffer.concat([
    Buffer.from([0xff, 0xd8]), seg(0xe0, [1, 2]), seg(0xe1, [9, 9, 9]), seg(0xdd, [0, 4]), seg(0xdb, [5]),
    Buffer.from([0xff, 0xda, 0, 2, 0x11, 0x22, 0xff, 0xd9]),
  ]);
  assert.deepEqual(jpegMarkers(cleanBaselineJpeg(src)), [0xd8, 0xe0, 0xdb, 0xda]);
});
