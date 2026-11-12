import { test } from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { setTimeout as sleep } from 'node:timers/promises';
import { Engine } from '../src/engine.js';
import { normalizeConfig } from '../src/config/schema.js';

const key = (action, extra = {}) => ({ action, ...extra });

function makeConfig() {
  return normalizeConfig({
    defaultProfileId: 'A', activeProfileId: 'A',
    profiles: [
      { id: 'A', name: 'A', keys: { 1: key({ type: 'keys', codes: ['KeyQ'] }), 2: key({ type: 'profile', profileId: 'B' }), 3: key({ type: 'profile', profileId: 'gone' }), 4: key({ type: 'none' }, { label: 'four' }) } },
      { id: 'B', name: 'B', processes: ['wow.exe'], keys: { 1: key({ type: 'keys', codes: ['KeyW'] }) } },
    ],
  });
}

function setup() {
  const device = new EventEmitter();
  device.images = [];
  device.setImages = (m) => device.images.push(new Map(m));
  device.setKeyMap = () => {};
  device.setBrightness = (b) => { device.brightness = b; };
  device.start = () => {};
  device.stop = async () => {};
  let saved = makeConfig();
  const store = {
    load: async () => structuredClone(saved),
    save: async (c) => { saved = structuredClone(c); },
    readImage: async () => null,
    pruneImages: async () => {},
  };
  const calls = [];
  const keyboard = { down: (c) => calls.push(['down', c]), up: (c) => calls.push(['up', c]), tapVk() {}, typeText() {} };
  const foreground = new EventEmitter();
  foreground.start = () => {};
  foreground.stop = () => {};
  const engine = new Engine({
    device, store, keyboard, foreground,
    launch() {}, system() {},
    render: async (k) => Buffer.from(JSON.stringify(k)),
    listProcesses: async () => [],
    log: { info() {}, warn() {}, error() {} },
  });
  return { engine, device, store, calls, foreground, getSaved: () => saved };
}

const flush = () => sleep(10);

test('connect renders all 15 keys', async () => {
  const { engine, device } = setup();
  await engine.start();
  device.emit('connected');
  await flush();
  assert.equal(device.images.at(-1).size, 15);
  assert.equal(engine.getState().connected, true);
});

test('deck key runs the active profile action', async () => {
  const { engine, device, calls } = setup();
  await engine.start();
  device.emit('key', { key: 1, pressed: true });
  device.emit('key', { key: 1, pressed: false });
  assert.deepEqual(calls, [['down', 'KeyQ'], ['up', 'KeyQ']]);
});

test('foreground game switches profile and back to the manual one', async () => {
  const { engine, device, foreground } = setup();
  await engine.start();
  device.emit('connected');
  await flush();
  foreground.emit('change', { exe: 'Wow.exe', elevated: false });
  await flush();
  assert.equal(engine.getState().activeProfileId, 'B');
  assert.deepEqual([...device.images.at(-1).keys()], [4]); // only tiles that differ from profile A
  foreground.emit('change', { exe: 'chrome.exe', elevated: false });
  await flush();
  assert.equal(engine.getState().activeProfileId, 'A');
  assert.equal(engine.getState().foregroundExe, 'chrome.exe');
});

test('disconnect releases held keys', async () => {
  const { engine, device, calls } = setup();
  await engine.start();
  device.emit('key', { key: 1, pressed: true });
  device.emit('disconnected');
  assert.deepEqual(calls, [['down', 'KeyQ'], ['up', 'KeyQ']]);
  assert.equal(engine.getState().connected, false);
});

test('setConfig re-renders only changed keys and persists', async () => {
  const { engine, device, getSaved } = setup();
  await engine.start();
  device.emit('connected');
  await flush();
  const cfg = structuredClone(engine.getConfig());
  cfg.profiles[0].keys[4].label = 'changed';
  await engine.setConfig(cfg);
  await flush();
  assert.deepEqual([...device.images.at(-1).keys()], [4]);
  assert.equal(getSaved().profiles[0].keys[4].label, 'changed');
});

test('profile action switches (manual); dangling target is a no-op', async () => {
  const { engine, device, getSaved } = setup();
  await engine.start();
  device.emit('key', { key: 3, pressed: true });
  await flush();
  assert.equal(engine.getState().activeProfileId, 'A');
  device.emit('key', { key: 2, pressed: true });
  await flush();
  assert.equal(engine.getState().activeProfileId, 'B');
  assert.equal(engine.getState().manualProfileId, 'B');
  assert.equal(getSaved().activeProfileId, 'B');
});

test('__back returns to the previous profile', async () => {
  const { engine } = setup();
  await engine.start();
  await engine.setActiveProfile('B');
  await engine.switchProfile('__back');
  assert.equal(engine.getState().activeProfileId, 'A');
});

test('deleting the active profile falls back to the default', async () => {
  const { engine } = setup();
  await engine.start();
  await engine.setActiveProfile('B');
  const cfg = structuredClone(engine.getConfig());
  cfg.profiles = cfg.profiles.filter((p) => p.id !== 'B');
  await engine.setConfig(cfg);
  assert.equal(engine.getState().activeProfileId, 'A');
});

test('calibration mode emits raw but runs no actions', async () => {
  const { engine, device, calls } = setup();
  await engine.start();
  engine.setCalibrating(true);
  const raws = [];
  engine.on('raw', (r) => raws.push(r));
  device.emit('raw', { deviceId: 1, pressed: true });
  device.emit('key', { key: 1, pressed: true });
  assert.deepEqual(calls, []);
  assert.deepEqual(raws, [{ deviceId: 1, pressed: true }]);
});
