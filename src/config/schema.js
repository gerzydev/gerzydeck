// Config shape, validation/normalization and the auto-switch decision.
import { SCANCODES, MEDIA_VK } from '../input/scancodes.js';

export const CONFIG_VERSION = 1;
export const KEY_COUNT = 15;
export const DEFAULT_BG = '#141618';
const MEDIA_COMMANDS = Object.keys(MEDIA_VK);
const SYSTEM_COMMANDS = ['lock', 'sleep'];
const IMAGE_ID = /^[0-9a-f]{16}\.png$/;
const COLOR = /^#[0-9a-f]{6}$/i;

const isObj = (v) => v !== null && typeof v === 'object' && !Array.isArray(v);
const str = (v, max = 200) => (typeof v === 'string' ? v.slice(0, max) : '');
export const newId = () => 'p' + globalThis.crypto.randomUUID().replace(/-/g, '').slice(0, 10);

export const emptyKey = () => ({ image: null, label: '', bg: DEFAULT_BG, action: { type: 'none' } });

function emptyKeys() {
  const keys = {};
  for (let k = 1; k <= KEY_COUNT; k++) keys[k] = emptyKey();
  return keys;
}

export function createProfile(name, id = newId()) {
  return { id, name, processes: [], keys: emptyKeys() };
}

export function createDefaultConfig() {
  const p = createProfile('Výchozí');
  return { version: CONFIG_VERSION, brightness: 80, keyMap: null, defaultProfileId: p.id, activeProfileId: p.id, profiles: [p] };
}

const validCode = (c) => typeof c === 'string' && Object.hasOwn(SCANCODES, c);

function normalizeStep(s) {
  if (!isObj(s)) return null;
  switch (s.type) {
    case 'down': case 'up': case 'tap':
      return validCode(s.code) ? { type: s.type, code: s.code } : null;
    case 'wait':
      return Number.isInteger(s.ms) && s.ms >= 0 && s.ms <= 60000 ? { type: 'wait', ms: s.ms } : null;
    case 'text':
      return typeof s.text === 'string' && s.text.length > 0 ? { type: 'text', text: s.text.slice(0, 1000) } : null;
    default:
      return null;
  }
}

export function normalizeAction(a) {
  const none = { type: 'none' };
  if (!isObj(a)) return none;
  switch (a.type) {
    case 'keys': {
      if (!Array.isArray(a.codes) || a.codes.length === 0 || a.codes.length > 6 || !a.codes.every(validCode)) return none;
      return { type: 'keys', codes: [...new Set(a.codes)] };
    }
    case 'macro': {
      if (!Array.isArray(a.steps) || a.steps.length === 0 || a.steps.length > 200) return none;
      const steps = a.steps.map(normalizeStep);
      return steps.every(Boolean) ? { type: 'macro', steps } : none;
    }
    case 'launch': {
      const target = str(a.target, 1000).trim();
      if (!target) return none;
      const args = Array.isArray(a.args) ? a.args.filter((x) => typeof x === 'string').slice(0, 50) : [];
      return { type: 'launch', target, args };
    }
    case 'media':
      return MEDIA_COMMANDS.includes(a.command) ? { type: 'media', command: a.command } : none;
    case 'system':
      return SYSTEM_COMMANDS.includes(a.command) ? { type: 'system', command: a.command } : none;
    case 'profile': {
      const profileId = str(a.profileId, 64);
      return profileId ? { type: 'profile', profileId } : none;
    }
    default:
      return none;
  }
}

function normalizeKey(k) {
  if (!isObj(k)) return emptyKey();
  return {
    image: typeof k.image === 'string' && IMAGE_ID.test(k.image) ? k.image : null,
    label: str(k.label, 40),
    bg: typeof k.bg === 'string' && COLOR.test(k.bg) ? k.bg.toLowerCase() : DEFAULT_BG,
    action: normalizeAction(k.action),
  };
}

function normalizeProfile(p, usedIds) {
  let id = str(p.id, 64);
  if (!id || usedIds.has(id)) id = newId();
  usedIds.add(id);
  const keys = {};
  const src = isObj(p.keys) ? p.keys : {};
  for (let k = 1; k <= KEY_COUNT; k++) keys[k] = normalizeKey(src[k]);
  const processes = Array.isArray(p.processes)
    ? [...new Set(p.processes.filter((x) => typeof x === 'string').map((x) => x.trim().toLowerCase()).filter(Boolean))]
    : [];
  return { id, name: str(p.name, 60).trim() || 'Profil', processes, keys };
}

function isPermutation(map) {
  return Array.isArray(map) && map.length === KEY_COUNT
    && [...map].sort((a, b) => a - b).every((v, i) => v === i + 1);
}

export function normalizeConfig(raw) {
  if (!isObj(raw) || !Array.isArray(raw.profiles)) return createDefaultConfig();
  const used = new Set();
  const profiles = raw.profiles.filter(isObj).map((p) => normalizeProfile(p, used));
  if (profiles.length === 0) return createDefaultConfig();
  const exists = (id) => profiles.some((p) => p.id === id);
  const defaultProfileId = exists(raw.defaultProfileId) ? raw.defaultProfileId : profiles[0].id;
  const activeProfileId = exists(raw.activeProfileId) ? raw.activeProfileId : defaultProfileId;
  const b = Number(raw.brightness);
  return {
    version: CONFIG_VERSION,
    brightness: Number.isFinite(b) ? Math.max(0, Math.min(100, Math.round(b))) : 80,
    keyMap: isPermutation(raw.keyMap) ? [...raw.keyMap] : null,
    defaultProfileId,
    activeProfileId,
    profiles,
  };
}

/** Which profile should be active for the given foreground exe. */
export function decideProfile(config, exe, manualProfileId) {
  if (exe) {
    const name = exe.toLowerCase();
    const hit = config.profiles.find((p) => p.processes.includes(name));
    if (hit) return hit.id;
  }
  if (config.profiles.some((p) => p.id === manualProfileId)) return manualProfileId;
  return config.defaultProfileId;
}
