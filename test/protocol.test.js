import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  connectPacket, wakePacket, modPacket, clearPacket, pixmapEndPacket, stopPacket,
  brightnessPacket, keyToSlot, imageUploadPackets, initPackets, BATCH_END,
  withReportId, parseInputReport,
} from '../src/deck/protocol.js';

const crt = (s) => Buffer.from('CRT\0\0' + s, 'latin1');

test('fixed packets are 1024 bytes with exact prefixes', () => {
  assert.equal(connectPacket().length, 1024);
  assert.deepEqual(connectPacket().subarray(0, 15), crt('CONNECT\0\0\0'));
  assert.deepEqual(wakePacket().subarray(0, 11), crt('DIS\0\0\x01'));
  assert.deepEqual(modPacket().subarray(0, 11), crt('MOD\0\0\x01'));
  assert.deepEqual(clearPacket().subarray(0, 12), crt('CLE\0\0\0\xff'));
  assert.deepEqual(pixmapEndPacket().subarray(0, 14), crt('PIXMAPEND'));
  assert.ok(stopPacket().subarray(10).every((b) => b === 0));
});

test('brightness clamps', () => {
  assert.equal(brightnessPacket(150)[10], 100);
  assert.equal(brightnessPacket(-5)[10], 0);
  assert.deepEqual(brightnessPacket(42).subarray(0, 11), Buffer.concat([crt('LIG\0\0'), Buffer.from([42])]));
});

test('slot map', () => {
  assert.deepEqual([1, 5, 6, 10, 11, 15].map(keyToSlot), [11, 15, 6, 10, 1, 5]);
});

test('image upload framing', () => {
  const jpeg = Buffer.alloc(2500, 7);
  const p = imageUploadPackets(1, jpeg);
  assert.equal(p.length, 1 + 3 + 1);
  assert.deepEqual(p[0].subarray(0, 13), Buffer.concat([crt('BAT\0\0'), Buffer.from([0x09, 0xC4, 11])]));
  assert.equal(p[3][2500 - 2048 - 1], 7);
  assert.equal(p[3][2500 - 2048], 0);
  assert.deepEqual(p[4].subarray(0, 8), crt('STP'));
  assert.ok(p.every((x) => x.length === 1024));
});

test('init sequence and batch end', () => {
  const names = initPackets(55).map((p) => p.subarray(5, 9).toString('latin1').replace(/\0/g, ''));
  assert.deepEqual(names, ['CONN', 'BLEF', 'MOD', 'DIS', 'LIG', 'CLE', 'STP']);
  assert.equal(initPackets(55)[4][10], 55);
  assert.deepEqual(BATCH_END().map((p) => p.subarray(5, 8).toString('latin1')), ['PIX', 'STP']);
});

test('withReportId', () => {
  const w = withReportId(connectPacket());
  assert.equal(w.length, 1025); assert.equal(w[0], 0); assert.equal(w[1], 0x43);
});

test('parse input: key down/up, ack, garbage, leading report-id byte', () => {
  const r = (id, st) => Buffer.concat([Buffer.from('ACK\0\0OK\0\0', 'latin1'), Buffer.from([id, st, 0xff]), Buffer.alloc(500)]);
  assert.deepEqual(parseInputReport(r(3, 1)), { type: 'key', deviceId: 3, key: 3, pressed: true });
  assert.deepEqual(parseInputReport(r(3, 0)), { type: 'key', deviceId: 3, key: 3, pressed: false });
  assert.deepEqual(parseInputReport(r(0xff, 0xff)), { type: 'ack' });
  assert.equal(parseInputReport(Buffer.concat([Buffer.from([0]), r(15, 1)])).key, 15);
  assert.equal(parseInputReport(Buffer.from('nope')), null);
  assert.equal(parseInputReport(r(16, 1)), null);
  const flipped = [11, 12, 13, 14, 15, 6, 7, 8, 9, 10, 1, 2, 3, 4, 5];
  assert.equal(parseInputReport(r(1, 1), flipped).key, 11);
});
