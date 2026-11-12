import { test } from 'node:test';
import assert from 'node:assert/strict';
import { SCANCODES, MEDIA_VK } from '../src/input/scancodes.js';

test('scancode spot checks', () => {
  assert.deepEqual(SCANCODES.KeyA, { scan: 0x1e, ext: false });
  assert.deepEqual(SCANCODES.KeyZ, { scan: 0x2c, ext: false });
  assert.deepEqual(SCANCODES.KeyY, { scan: 0x15, ext: false });
  assert.deepEqual(SCANCODES.Digit1, { scan: 0x02, ext: false });
  assert.deepEqual(SCANCODES.Digit0, { scan: 0x0b, ext: false });
  assert.deepEqual(SCANCODES.F1, { scan: 0x3b, ext: false });
  assert.deepEqual(SCANCODES.F11, { scan: 0x57, ext: false });
  assert.deepEqual(SCANCODES.F13, { scan: 0x64, ext: false });
  assert.deepEqual(SCANCODES.F24, { scan: 0x76, ext: false });
  assert.deepEqual(SCANCODES.ArrowUp, { scan: 0x48, ext: true });
  assert.deepEqual(SCANCODES.ControlRight, { scan: 0x1d, ext: true });
  assert.deepEqual(SCANCODES.NumpadEnter, { scan: 0x1c, ext: true });
  assert.deepEqual(SCANCODES.MetaLeft, { scan: 0x5b, ext: true });
  assert.deepEqual(SCANCODES.Escape, { scan: 0x01, ext: false });
});

test('media vks', () => {
  assert.deepEqual(MEDIA_VK, { playpause: 0xb3, next: 0xb0, prev: 0xb1, mute: 0xad, volup: 0xaf, voldown: 0xae });
});
