import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createDefaultConfig, normalizeConfig, decideProfile } from '../src/config/schema.js';

const profile = (id, extra = {}) => ({ id, name: id, processes: [], keys: {}, ...extra });

test('default config is valid and has 15 empty keys', () => {
  const c = createDefaultConfig();
  assert.equal(c.version, 1);
  assert.equal(c.profiles.length, 1);
  assert.equal(c.profiles[0].name, 'Výchozí');
  assert.equal(Object.keys(c.profiles[0].keys).length, 15);
  assert.deepEqual(c.profiles[0].keys[7], { image: null, label: '', bg: '#141618', action: { type: 'none' } });
  assert.equal(c.activeProfileId, c.profiles[0].id);
  assert.deepEqual(normalizeConfig(c), c);
});

test('garbage input becomes a default config', () => {
  for (const raw of [null, 42, 'x', { profiles: 'nope' }, { profiles: [] }]) {
    const c = normalizeConfig(raw);
    assert.equal(c.profiles.length, 1);
    assert.equal(c.defaultProfileId, c.profiles[0].id);
  }
});

test('invalid actions become none, valid ones survive', () => {
  const keys = {
    1: { action: { type: 'explode' } },
    2: { action: { type: 'keys', codes: ['KeyQ', 'Bogus'] } },
    3: { action: { type: 'keys', codes: ['ShiftLeft', 'KeyQ'] } },
    4: { action: { type: 'macro', steps: [{ type: 'wait', ms: 999999 }] } },
    5: { action: { type: 'macro', steps: [{ type: 'tap', code: 'KeyQ' }, { type: 'wait', ms: 50 }, { type: 'text', text: 'ahoj' }] } },
    6: { action: { type: 'media', command: 'volup' } },
    7: { action: { type: 'system', command: 'format-c' } },
    8: { action: { type: 'system', command: 'lock' } },
    9: { action: { type: 'launch', target: 'notepad.exe' } },
    10: { action: { type: 'keys', codes: [] } },
  };
  const c = normalizeConfig({ profiles: [profile('a', { keys })] });
  const k = c.profiles[0].keys;
  assert.equal(k[1].action.type, 'none');
  assert.equal(k[2].action.type, 'none');
  assert.deepEqual(k[3].action, { type: 'keys', codes: ['ShiftLeft', 'KeyQ'] });
  assert.equal(k[4].action.type, 'none');
  assert.equal(k[5].action.steps.length, 3);
  assert.deepEqual(k[6].action, { type: 'media', command: 'volup' });
  assert.equal(k[7].action.type, 'none');
  assert.deepEqual(k[8].action, { type: 'system', command: 'lock' });
  assert.deepEqual(k[9].action, { type: 'launch', target: 'notepad.exe', args: [] });
  assert.equal(k[10].action.type, 'none');
  assert.equal(k[11].action.type, 'none');
});

test('dangling active/default ids are repaired; processes lower-cased; brightness clamped; bad keyMap dropped', () => {
  const c = normalizeConfig({
    activeProfileId: 'gone', defaultProfileId: 'gone', brightness: 300, keyMap: [1, 1, 1],
    profiles: [profile('a', { processes: ['Wow.exe', ' WOW.EXE '] }), profile('b')],
  });
  assert.equal(c.activeProfileId, 'a');
  assert.equal(c.defaultProfileId, 'a');
  assert.deepEqual(c.profiles[0].processes, ['wow.exe']);
  assert.equal(c.brightness, 100);
  assert.equal(c.keyMap, null);
  const ok = [15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1];
  assert.deepEqual(normalizeConfig({ keyMap: ok, profiles: [profile('a')] }).keyMap, ok);
});

test('duplicate profile ids are made unique', () => {
  const c = normalizeConfig({ profiles: [profile('a'), profile('a')] });
  assert.notEqual(c.profiles[0].id, c.profiles[1].id);
});

test('dangling profile action targets are kept (runner treats them as no-op)', () => {
  const c = normalizeConfig({ profiles: [profile('a', { keys: { 1: { action: { type: 'profile', profileId: 'gone' } } } })] });
  assert.deepEqual(c.profiles[0].keys[1].action, { type: 'profile', profileId: 'gone' });
});

test('decideProfile', () => {
  const c = normalizeConfig({
    defaultProfileId: 'def',
    profiles: [profile('def'), profile('wow1', { processes: ['wow.exe'] }), profile('wow2', { processes: ['wow.exe'] }), profile('manual')],
  });
  assert.equal(decideProfile(c, 'Wow.exe', 'manual'), 'wow1');
  assert.equal(decideProfile(c, 'chrome.exe', 'manual'), 'manual');
  assert.equal(decideProfile(c, null, 'manual'), 'manual');
  assert.equal(decideProfile(c, 'chrome.exe', 'deleted'), 'def');
});
