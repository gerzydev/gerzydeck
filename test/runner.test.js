import { test } from 'node:test';
import assert from 'node:assert/strict';
import { setTimeout as sleep } from 'node:timers/promises';
import { createActionRunner } from '../src/actions/runner.js';

function setup() {
  const calls = [];
  const keyboard = {
    down: (c) => calls.push(['down', c]),
    up: (c) => calls.push(['up', c]),
    tapVk: (vk) => calls.push(['vk', vk]),
    typeText: (t) => calls.push(['text', t]),
  };
  const switched = [];
  const launched = [];
  const system = [];
  const runner = createActionRunner({
    keyboard,
    launch: (t, a) => launched.push([t, a]),
    system: (c) => system.push(c),
    switchProfile: (id) => switched.push(id),
    log: { warn() {}, error() {} },
  });
  return { calls, switched, launched, system, runner };
}

test('keys: press downs in order, release ups in reverse', () => {
  const { calls, runner } = setup();
  runner.press(1, { type: 'keys', codes: ['ShiftLeft', 'Digit3'] });
  assert.deepEqual(calls, [['down', 'ShiftLeft'], ['down', 'Digit3']]);
  runner.release(1);
  assert.deepEqual(calls.slice(2), [['up', 'Digit3'], ['up', 'ShiftLeft']]);
});

test('release lifts the codes pressed earlier even after a profile switch', () => {
  const { calls, runner } = setup();
  runner.press(1, { type: 'keys', codes: ['ShiftLeft'] });
  runner.press(2, { type: 'profile', profileId: 'p2' }); // config changes under our feet
  runner.release(1);
  assert.deepEqual(calls, [['down', 'ShiftLeft'], ['up', 'ShiftLeft']]);
});

test('macro runs steps in order, waits, auto-releases held downs, ignores re-press', async () => {
  const { calls, runner } = setup();
  const action = { type: 'macro', steps: [{ type: 'tap', code: 'KeyQ' }, { type: 'wait', ms: 30 }, { type: 'down', code: 'KeyE' }] };
  const t0 = Date.now();
  const done = runner.press(3, action);
  assert.equal(runner.isMacroRunning(3), true);
  runner.press(3, action);
  await done;
  assert.ok(Date.now() - t0 >= 28);
  assert.deepEqual(calls, [['down', 'KeyQ'], ['up', 'KeyQ'], ['down', 'KeyE'], ['up', 'KeyE']]);
  assert.equal(runner.isMacroRunning(3), false);
});

test('macro text step', async () => {
  const { calls, runner } = setup();
  await runner.press(1, { type: 'macro', steps: [{ type: 'text', text: 'ěšč' }] });
  assert.deepEqual(calls, [['text', 'ěšč']]);
});

test('releaseAll lifts everything held', () => {
  const { calls, runner } = setup();
  runner.press(1, { type: 'keys', codes: ['KeyW'] });
  runner.press(2, { type: 'keys', codes: ['KeyA'] });
  runner.releaseAll();
  assert.deepEqual(calls.slice(2).sort(), [['up', 'KeyA'], ['up', 'KeyW']]);
  runner.release(1);
  assert.equal(calls.length, 4);
});

test('profile, launch, media, system fire on press only', () => {
  const { calls, switched, launched, system, runner } = setup();
  runner.press(1, { type: 'profile', profileId: 'p2' });
  runner.press(2, { type: 'launch', target: 'notepad.exe', args: ['a.txt'] });
  runner.press(3, { type: 'media', command: 'volup' });
  runner.press(4, { type: 'system', command: 'lock' });
  [1, 2, 3, 4].forEach((k) => runner.release(k));
  assert.deepEqual(switched, ['p2']);
  assert.deepEqual(launched, [['notepad.exe', ['a.txt']]]);
  assert.deepEqual(calls, [['vk', 0xaf]]);
  assert.deepEqual(system, ['lock']);
});

test('a failing backend does not throw out of press', async () => {
  const { runner } = setup();
  const r = createActionRunner({
    keyboard: { down() { throw new Error('boom'); }, up() {}, tapVk() {}, typeText() {} },
    launch() {}, system() {}, switchProfile() {}, log: { warn() {}, error() {} },
  });
  assert.doesNotThrow(() => r.press(1, { type: 'keys', codes: ['KeyQ'] }));
  await sleep(0);
  runner.releaseAll();
});
