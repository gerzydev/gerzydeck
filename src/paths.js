import path from 'node:path';
import os from 'node:os';

export const dataDir = () =>
  process.env.GERZYS_DECK_DIR || path.join(process.env.APPDATA || path.join(os.homedir(), 'AppData', 'Roaming'), 'GerzysDeck');
export const configPath = () => path.join(dataDir(), 'config.json');
export const imagesDir = () => path.join(dataDir(), 'images');
export const logPath = () => path.join(dataDir(), 'log.txt');
