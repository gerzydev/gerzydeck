// Engine: owns config + active profile, renders the deck, routes presses to actions.
import { EventEmitter } from 'node:events';
import { normalizeConfig, decideProfile } from './config/schema.js';
import { createActionRunner } from './actions/runner.js';
import { renderKeyJpeg } from './deck/render.js';
import { findConflicts } from './os/processes.js';

const KEYS = Array.from({ length: 15 }, (_, i) => i + 1);

export class Engine extends EventEmitter {
  constructor({ device, store, keyboard, launch, system, foreground, render = renderKeyJpeg, listProcesses = async () => [], conflictPollMs = 10_000, log = console }) {
    super();
    Object.assign(this, { device, store, foreground, render, listProcessesFn: listProcesses, conflictPollMs, log });
    this.config = null;
    this.state = {
      connected: false, activeProfileId: null, manualProfileId: null,
      foregroundExe: null, foregroundElevated: false, conflicts: [], calibrating: false,
    };
    this.previousProfileId = null;
    this.rendered = new Map(); // key → signature currently on the deck
    this.renderGen = 0;
    this.runner = createActionRunner({
      keyboard, launch, system, log,
      switchProfile: (id) => this.switchProfile(id),
    });
  }

  async start() {
    this.config = await this.store.load();
    this.state.activeProfileId = this.config.activeProfileId;
    this.state.manualProfileId = this.config.activeProfileId;
    this.device.setKeyMap(this.config.keyMap);
    this.device.brightness = this.config.brightness;

    this.device.on('connected', () => {
      this.state.connected = true;
      this.rendered.clear();
      this.#emitState();
      this.#renderActive();
    });
    this.device.on('disconnected', () => {
      this.runner.releaseAll();
      this.state.connected = false;
      this.#emitState();
    });
    this.device.on('raw', (ev) => this.emit('raw', ev));
    this.device.on('key', (ev) => this.#onKey(ev));
    this.foreground.on('change', ({ exe, elevated }) => {
      this.state.foregroundExe = exe;
      this.state.foregroundElevated = elevated;
      const target = decideProfile(this.config, exe, this.state.manualProfileId);
      if (target !== this.state.activeProfileId) this.#activate(target);
      else this.#emitState();
    });

    this.device.start();
    this.foreground.start();
    this.#pollConflicts();
    this.conflictTimer = setInterval(() => this.#pollConflicts(), this.conflictPollMs);
    this.conflictTimer.unref?.();
  }

  async stop() {
    clearInterval(this.conflictTimer);
    this.runner.releaseAll();
    this.foreground.stop();
    await this.device.stop();
  }

  getConfig() { return this.config; }
  getState() { return { ...this.state }; }
  listProcesses() { return this.listProcessesFn(); }
  activeProfile() { return this.config.profiles.find((p) => p.id === this.state.activeProfileId) ?? this.config.profiles[0]; }

  setCalibrating(on) {
    this.state.calibrating = Boolean(on);
    if (on) this.runner.releaseAll();
    this.#emitState();
  }

  async setConfig(raw) {
    const cfg = normalizeConfig(raw);
    const prev = this.config;
    this.config = cfg;
    await this.store.save(cfg);
    this.store.pruneImages(cfg).catch((err) => this.log.warn?.('úklid obrázků selhal:', err.message));
    if (cfg.brightness !== prev.brightness) this.device.setBrightness(cfg.brightness);
    if (JSON.stringify(cfg.keyMap) !== JSON.stringify(prev.keyMap)) this.device.setKeyMap(cfg.keyMap);

    const exists = (id) => cfg.profiles.some((p) => p.id === id);
    if (!exists(this.state.manualProfileId)) this.state.manualProfileId = cfg.defaultProfileId;
    if (!exists(this.previousProfileId)) this.previousProfileId = null;
    const target = exists(this.state.activeProfileId)
      ? this.state.activeProfileId
      : decideProfile(cfg, this.state.foregroundExe, this.state.manualProfileId);
    this.emit('config', cfg);
    if (target !== this.state.activeProfileId) this.#activate(target);
    else this.#renderActive();
    return cfg;
  }

  /** Manual switch (web UI, deck profile action). */
  async setActiveProfile(id) {
    if (!this.config.profiles.some((p) => p.id === id)) return false;
    this.state.manualProfileId = id;
    if (this.config.activeProfileId !== id) {
      this.config = { ...this.config, activeProfileId: id };
      this.store.save(this.config).catch((err) => this.log.warn?.('uložení selhalo:', err.message));
    }
    if (id !== this.state.activeProfileId) this.#activate(id);
    return true;
  }

  async switchProfile(id) {
    const target = id === '__back' ? this.previousProfileId : id;
    if (!target) return false;
    return this.setActiveProfile(target);
  }

  #activate(id) {
    this.previousProfileId = this.state.activeProfileId;
    this.state.activeProfileId = id;
    this.#emitState();
    this.#renderActive();
  }

  #onKey({ key, pressed }) {
    this.emit('key', { key, pressed });
    if (this.state.calibrating) return;
    if (pressed) {
      const action = this.activeProfile().keys[key]?.action;
      const r = this.runner.press(key, action);
      if (r?.catch) r.catch((err) => this.log.error?.('akce selhala:', err));
    } else {
      this.runner.release(key);
    }
  }

  async #renderActive() {
    if (!this.state.connected) return;
    const gen = ++this.renderGen;
    const profile = this.activeProfile();
    const images = new Map();
    for (const k of KEYS) {
      const cfg = profile.keys[k];
      const sig = `${cfg.image}|${cfg.label}|${cfg.bg}`;
      if (this.rendered.get(k) === sig) continue;
      try {
        const image = cfg.image ? await this.store.readImage(cfg.image) : null;
        images.set(k, { sig, jpeg: await this.render({ image, label: cfg.label, bg: cfg.bg }) });
      } catch (err) {
        this.log.error?.(`vykreslení klávesy ${k} selhalo:`, err.message);
      }
      if (gen !== this.renderGen) return; // superseded by a newer render
    }
    if (!images.size || !this.state.connected) return;
    for (const [k, { sig }] of images) this.rendered.set(k, sig);
    this.device.setImages(new Map([...images].map(([k, v]) => [k, v.jpeg])));
  }

  async #pollConflicts() {
    try {
      const conflicts = findConflicts(await this.listProcessesFn());
      if (conflicts.join() !== this.state.conflicts.join()) {
        this.state.conflicts = conflicts;
        this.#emitState();
      }
    } catch { /* ignore */ }
  }

  #emitState() { this.emit('state', this.getState()); }
}
