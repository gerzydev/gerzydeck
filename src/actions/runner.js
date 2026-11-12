// Executes key actions. Backends (keyboard, launch, system, profile switch) are injected.
import { setTimeout as sleep } from 'node:timers/promises';
import { MEDIA_VK } from '../input/scancodes.js';

export function createActionRunner({ keyboard, launch, system, switchProfile, log = console }) {
  const held = new Map();      // deck key → codes pressed by a `keys` action
  const macros = new Set();    // deck keys whose macro is running

  const safe = (what, fn) => {
    try { return fn(); } catch (err) { log.error?.(`akce ${what} selhala:`, err); return undefined; }
  };

  async function runMacro(key, steps) {
    macros.add(key);
    const down = new Set();
    try {
      for (const s of steps) {
        switch (s.type) {
          case 'tap': keyboard.down(s.code); keyboard.up(s.code); break;
          case 'down': keyboard.down(s.code); down.add(s.code); break;
          case 'up': keyboard.up(s.code); down.delete(s.code); break;
          case 'wait': await sleep(s.ms); break;
          case 'text': keyboard.typeText(s.text); break;
        }
      }
    } catch (err) {
      log.error?.('makro selhalo:', err);
    } finally {
      for (const code of [...down].reverse()) safe('up', () => keyboard.up(code));
      macros.delete(key);
    }
  }

  function press(key, action) {
    switch (action?.type) {
      case 'keys': {
        if (held.has(key)) return undefined;
        const pressed = [];
        held.set(key, pressed);
        for (const code of action.codes) {
          if (safe('down', () => { keyboard.down(code); return true; })) pressed.push(code);
        }
        return undefined;
      }
      case 'macro':
        if (macros.has(key)) return undefined;
        return runMacro(key, action.steps);
      case 'launch': return safe('launch', () => launch(action.target, action.args ?? []));
      case 'media': return safe('media', () => keyboard.tapVk(MEDIA_VK[action.command]));
      case 'system': return safe('system', () => system(action.command));
      case 'profile': return safe('profile', () => switchProfile(action.profileId));
      default: return undefined;
    }
  }

  function release(key) {
    const codes = held.get(key);
    if (!codes) return;
    held.delete(key);
    for (const code of [...codes].reverse()) safe('up', () => keyboard.up(code));
  }

  function releaseAll() {
    for (const key of [...held.keys()]) release(key);
  }

  return { press, release, releaseAll, isMacroRunning: (key) => macros.has(key) };
}
