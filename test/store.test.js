import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { createStore } from '../src/config/store.js';

const tmp = () => fs.mkdtemp(path.join(os.tmpdir(), 'deck-'));

test('missing config → default, saved to disk', async () => {
  const dir = await tmp();
  const s = createStore(dir);
  const c = await s.load();
  assert.equal(c.profiles.length, 1);
  const onDisk = JSON.parse(await fs.readFile(path.join(dir, 'config.json'), 'utf8'));
  assert.equal(onDisk.version, 1);
});

test('round trip', async () => {
  const s = createStore(await tmp());
  const c = await s.load();
  c.profiles[0].name = 'WoW';
  await s.save(c);
  assert.equal((await s.load()).profiles[0].name, 'WoW');
});

test('corrupt config is backed up and defaults load', async () => {
  const dir = await tmp();
  await fs.writeFile(path.join(dir, 'config.json'), '{ nope');
  const c = await createStore(dir).load();
  assert.equal(c.profiles.length, 1);
  const files = await fs.readdir(dir);
  assert.ok(files.some((f) => /^config\.broken-\d+\.json$/.test(f)), files.join(','));
});

test('images: save, read, traversal guard, prune', async () => {
  const dir = await tmp();
  const s = createStore(dir);
  const c = await s.load();
  const keep = await s.saveImage(Buffer.from('png-a'));
  const drop = await s.saveImage(Buffer.from('png-b'));
  assert.match(keep, /^[0-9a-f]{16}\.png$/);
  assert.deepEqual(await s.readImage(keep), Buffer.from('png-a'));
  assert.equal(await s.readImage('../../x'), null);
  assert.equal(await s.readImage('0123456789abcdef.png'), null);
  c.profiles[0].keys[1].image = keep;
  await s.pruneImages(c);
  assert.ok(await s.readImage(keep));
  assert.equal(await s.readImage(drop), null);
});
