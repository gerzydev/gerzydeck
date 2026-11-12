// node-hid adapter for the D6: discovery, reconnect, paced writes, heartbeat, input events.
import { EventEmitter } from 'node:events';
import { setTimeout as sleep } from 'node:timers/promises';
import nodeHid from 'node-hid';
import {
  VENDOR_ID, PRODUCT_ID, USAGE_PAGE, DEFAULT_KEY_MAP,
  initPackets, imageUploadPackets, BATCH_END, brightnessPacket, stopPacket,
  connectPacket, sleepPacket, withReportId, parseInputReport,
} from './protocol.js';

export class DeckDevice extends EventEmitter {
  constructor({
    keyMap = null, reportIdPrefix = true, writeDelayMs = 5, heartbeatMs = 2000,
    reconnectMs = 2000, brightness = 80, hid = nodeHid,
  } = {}) {
    super();
    this.hid = hid;
    this.keyMap = keyMap ?? DEFAULT_KEY_MAP;
    this.reportIdPrefix = reportIdPrefix;
    this.writeDelayMs = writeDelayMs;
    this.heartbeatMs = heartbeatMs;
    this.reconnectMs = reconnectMs;
    this.brightness = brightness;
    this.dev = null;
    this.running = false;
    this.queue = [];               // control packets, sent first
    this.pendingImages = new Map(); // key → jpeg, merged until sent
    this.pumping = false;
    this.pollTimer = null;
    this.heartbeat = null;
  }

  get isConnected() { return this.dev !== null; }

  start() {
    if (this.running) return;
    this.running = true;
    this.#poll();
  }

  async stop() {
    this.running = false;
    clearTimeout(this.pollTimer);
    clearInterval(this.heartbeat);
    const dev = this.dev;
    if (!dev) return;
    this.queue = [];
    this.pendingImages.clear();
    try { await dev.write(this.#frame(sleepPacket())); } catch { /* already gone */ }
    this.dev = null;
    try { await dev.close(); } catch { /* ignore */ }
  }

  setKeyMap(map) { this.keyMap = map ?? DEFAULT_KEY_MAP; }

  setBrightness(level) {
    this.brightness = level;
    this.#enqueue([brightnessPacket(level), stopPacket()]);
  }

  setImages(images) {
    for (const [key, jpeg] of images) this.pendingImages.set(key, jpeg);
    this.#kick();
  }

  async #poll() {
    if (!this.running || this.dev) return;
    try {
      const list = await this.hid.devicesAsync();
      const info = list.find((d) => d.vendorId === VENDOR_ID && d.productId === PRODUCT_ID && d.usagePage === USAGE_PAGE);
      if (info && this.running) {
        const dev = await this.hid.HIDAsync.open(info.path);
        this.#attach(dev);
        return;
      }
    } catch (err) {
      if (this.listenerCount('error')) this.emit('error', err);
    }
    if (this.running) this.pollTimer = setTimeout(() => this.#poll(), this.reconnectMs);
  }

  #attach(dev) {
    this.dev = dev;
    dev.on('data', (buf) => this.#onData(buf));
    dev.on('error', (err) => this.#drop(dev, err));
    this.queue = [...initPackets(this.brightness)];
    this.heartbeat = setInterval(() => this.#enqueue([connectPacket()]), this.heartbeatMs);
    this.#kick();
    this.emit('connected');
  }

  #drop(dev, err) {
    if (this.dev !== dev) return;
    this.dev = null;
    clearInterval(this.heartbeat);
    this.queue = [];
    this.pendingImages.clear();
    dev.close?.().catch?.(() => {});
    if (err && this.listenerCount('error')) this.emit('error', err);
    this.emit('disconnected');
    if (this.running) this.pollTimer = setTimeout(() => this.#poll(), this.reconnectMs);
  }

  #onData(buf) {
    const ev = parseInputReport(Buffer.from(buf), this.keyMap);
    if (!ev || ev.type !== 'key') return;
    this.emit('raw', { deviceId: ev.deviceId, pressed: ev.pressed });
    this.emit('key', { key: ev.key, pressed: ev.pressed });
  }

  #frame(pkt) { return this.reportIdPrefix ? withReportId(pkt) : pkt; }

  #enqueue(packets) {
    if (!this.dev) return;
    this.queue.push(...packets);
    this.#kick();
  }

  #kick() {
    if (this.pumping || !this.dev) return;
    this.pumping = true;
    setImmediate(() => this.#pump());
  }

  async #pump() {
    try {
      while (this.dev) {
        let pkt = this.queue.shift();
        if (!pkt && this.pendingImages.size) {
          const batch = [...this.pendingImages];
          this.pendingImages.clear();
          for (const [key, jpeg] of batch) this.queue.push(...imageUploadPackets(key, jpeg));
          this.queue.push(...BATCH_END());
          pkt = this.queue.shift();
        }
        if (!pkt) break;
        const dev = this.dev;
        try {
          await dev.write(this.#frame(pkt));
        } catch (err) {
          this.#drop(dev, err);
          break;
        }
        if (this.writeDelayMs) await sleep(this.writeDelayMs);
      }
    } finally {
      this.pumping = false;
    }
    if (this.dev && (this.queue.length || this.pendingImages.size)) this.#kick();
  }
}
