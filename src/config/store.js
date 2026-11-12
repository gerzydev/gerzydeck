// Persistence: config.json (atomic writes) + content-addressed key images.
import fs from 'node:fs/promises';
import path from 'node:path';
import crypto from 'node:crypto';
import { createDefaultConfig, normalizeConfig } from './schema.js';

const IMAGE_ID = /^[0-9a-f]{16}\.png$/;

export function createStore(dir) {
  const configFile = path.join(dir, 'config.json');
  const imagesDir = path.join(dir, 'images');

  async function writeAtomic(file, data) {
    await fs.mkdir(path.dirname(file), { recursive: true });
    const tmp = `${file}.tmp`;
    await fs.writeFile(tmp, data);
    await fs.rename(tmp, file);
  }

  async function save(cfg) {
    await writeAtomic(configFile, JSON.stringify(cfg, null, 2));
  }

  async function load() {
    let text;
    try {
      text = await fs.readFile(configFile, 'utf8');
    } catch (err) {
      if (err.code !== 'ENOENT') throw err;
      const cfg = createDefaultConfig();
      await save(cfg);
      return cfg;
    }
    try {
      return normalizeConfig(JSON.parse(text));
    } catch {
      await fs.rename(configFile, path.join(dir, `config.broken-${Date.now()}.json`));
      const cfg = createDefaultConfig();
      await save(cfg);
      return cfg;
    }
  }

  async function saveImage(png) {
    const id = crypto.createHash('sha256').update(png).digest('hex').slice(0, 16) + '.png';
    await fs.mkdir(imagesDir, { recursive: true });
    await writeAtomic(path.join(imagesDir, id), png);
    return id;
  }

  async function readImage(id) {
    if (typeof id !== 'string' || !IMAGE_ID.test(id)) return null;
    try {
      return await fs.readFile(path.join(imagesDir, id));
    } catch {
      return null;
    }
  }

  async function pruneImages(cfg) {
    const used = new Set(cfg.profiles.flatMap((p) => Object.values(p.keys).map((k) => k.image)).filter(Boolean));
    let files = [];
    try { files = await fs.readdir(imagesDir); } catch { return; }
    await Promise.all(files.filter((f) => IMAGE_ID.test(f) && !used.has(f)).map((f) => fs.rm(path.join(imagesDir, f), { force: true })));
  }

  return { load, save, saveImage, readImage, pruneImages };
}
