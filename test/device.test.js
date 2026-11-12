import { test } from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter, once } from 'node:events';
import { setTimeout as sleep } from 'node:timers/promises';
import { DeckDevice } from '../src/deck/device.js';

function fakeHid() {
  const opened = [];
  const hid = {
    opened,
    async devicesAsync() {
      return [
        { vendorId: 0x3142, productId: 0x0060, usagePage: 1, usage: 6, path: 'kbd' },
        { vendorId: 0x3142, productId: 0x0060, usagePage: 0xffa0, usage: 1, path: 'deck' },
      ];
    },
    HIDAsync: {
      async open(path) {
        const dev = new EventEmitter();
        dev.path = path;
        dev.writes = [];
        dev.write = async (buf) => { dev.writes.push(Buffer.from(buf)); return buf.length; };
        dev.close = async () => { dev.closed = true; };
        opened.push(dev);
        return dev;
      },
    },
  };
  return hid;
}

const keyReport = (id, st) => Buffer.concat([Buffer.from('ACK\0\0OK\0\0', 'latin1'), Buffer.from([id, st, 0xff]), Buffer.alloc(500)]);
const opts = (hid) => ({ hid, writeDelayMs: 0, heartbeatMs: 20, reconnectMs: 10 });

test('opens the vendor interface and sends the init sequence', async () => {
  const hid = fakeHid();
  const d = new DeckDevice(opts(hid));
  const connected = once(d, 'connected');
  d.start();
  await connected;
  await sleep(5);
  const dev = hid.opened[0];
  assert.equal(dev.path, 'deck');
  assert.ok(dev.writes.length >= 7);
  assert.ok(dev.writes.every((w) => w.length === 1025 && w[0] === 0));
  assert.equal(dev.writes[0][1], 0x43);
  assert.equal(d.isConnected, true);
  await d.stop();
});

test('emits key and raw events', async () => {
  const hid = fakeHid();
  const d = new DeckDevice(opts(hid));
  d.start();
  await once(d, 'connected');
  const key = once(d, 'key');
  const raw = once(d, 'raw');
  hid.opened[0].emit('data', keyReport(2, 1));
  assert.deepEqual((await key)[0], { key: 2, pressed: true });
  assert.deepEqual((await raw)[0], { deviceId: 2, pressed: true });
  await d.stop();
});

test('drops on error and reconnects', async () => {
  const hid = fakeHid();
  const d = new DeckDevice(opts(hid));
  d.start();
  await once(d, 'connected');
  const errors = [];
  d.on('error', (e) => errors.push(e.message));
  const disc = new Promise((r) => d.once('disconnected', r));
  hid.opened[0].emit('error', new Error('unplugged'));
  await disc;
  assert.equal(d.isConnected, false);
  assert.deepEqual(errors, ['unplugged']);
  await once(d, 'connected');
  assert.equal(hid.opened.length, 2);
  await d.stop();
});

test('sends heartbeats', async () => {
  const hid = fakeHid();
  const d = new DeckDevice(opts(hid));
  d.start();
  await once(d, 'connected');
  await sleep(70);
  const connects = hid.opened[0].writes.filter((w) => w.toString('latin1', 6, 13) === 'CONNECT');
  assert.ok(connects.length >= 3, `got ${connects.length}`);
  await d.stop();
});

test('setImages sends one batch ending with PIXMAPEND, latest image wins', async () => {
  const hid = fakeHid();
  const d = new DeckDevice({ ...opts(hid), heartbeatMs: 10_000 });
  d.start();
  await once(d, 'connected');
  await sleep(5);
  const dev = hid.opened[0];
  dev.writes.length = 0;
  d.setImages(new Map([[1, Buffer.alloc(10, 1)]]));
  d.setImages(new Map([[1, Buffer.alloc(10, 2)], [2, Buffer.alloc(10, 3)]]));
  await sleep(20);
  const bats = dev.writes.filter((w) => w.toString('latin1', 6, 9) === 'BAT');
  assert.equal(bats.length, 2);
  const pix = dev.writes.filter((w) => w.toString('latin1', 6, 15) === 'PIXMAPEND');
  assert.equal(pix.length, 1);
  assert.ok(dev.writes.some((w) => w[1] === 2 && w[10] === 2));
  await d.stop();
});
