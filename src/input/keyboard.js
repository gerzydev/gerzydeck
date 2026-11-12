// SendInput adapter (user32 via koffi). Keys go out as scan codes so games see them.
import koffi from 'koffi';
import { SCANCODES } from './scancodes.js';

const INPUT_KEYBOARD = 1;
const KEYEVENTF_EXTENDEDKEY = 0x0001;
const KEYEVENTF_KEYUP = 0x0002;
const KEYEVENTF_UNICODE = 0x0004;
const KEYEVENTF_SCANCODE = 0x0008;

let api = null;
function load() {
  if (api) return api;
  const user32 = koffi.load('user32.dll');
  const KEYBDINPUT = koffi.struct('KEYBDINPUT', {
    wVk: 'uint16', wScan: 'uint16', dwFlags: 'uint32', time: 'uint32', dwExtraInfo: 'uintptr_t',
  });
  const MOUSEINPUT = koffi.struct('MOUSEINPUT', {
    dx: 'int32', dy: 'int32', mouseData: 'uint32', dwFlags: 'uint32', time: 'uint32', dwExtraInfo: 'uintptr_t',
  });
  const HARDWAREINPUT = koffi.struct('HARDWAREINPUT', { uMsg: 'uint32', wParamL: 'uint16', wParamH: 'uint16' });
  const INPUT_UNION = koffi.union('INPUT_UNION', { mi: MOUSEINPUT, ki: KEYBDINPUT, hi: HARDWAREINPUT });
  const INPUT = koffi.struct('INPUT', { type: 'uint32', u: INPUT_UNION });
  const size = koffi.sizeof(INPUT);
  if (size !== 40) throw new Error(`unexpected INPUT size ${size}`);
  const SendInput = user32.func('__stdcall', 'SendInput', 'uint32', ['uint32', koffi.pointer(INPUT), 'int32']);
  api = { SendInput, size };
  return api;
}

function send(ki) {
  const { SendInput, size } = load();
  const input = { type: INPUT_KEYBOARD, u: { ki: { wVk: 0, wScan: 0, dwFlags: 0, time: 0, dwExtraInfo: 0, ...ki } } };
  const sent = SendInput(1, [input], size);
  if (sent !== 1) throw new Error('SendInput was blocked (UIPI / elevated window?)');
}

function scan(code, up) {
  const sc = SCANCODES[code];
  if (!sc) throw new Error(`unknown key code ${code}`);
  send({ wScan: sc.scan, dwFlags: KEYEVENTF_SCANCODE | (sc.ext ? KEYEVENTF_EXTENDEDKEY : 0) | (up ? KEYEVENTF_KEYUP : 0) });
}

export function createKeyboard() {
  load();
  return {
    down: (code) => scan(code, false),
    up: (code) => scan(code, true),
    tapVk(vk) {
      send({ wVk: vk, dwFlags: KEYEVENTF_EXTENDEDKEY });
      send({ wVk: vk, dwFlags: KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP });
    },
    typeText(text) {
      for (let i = 0; i < text.length; i++) {
        const unit = text.charCodeAt(i);
        send({ wScan: unit, dwFlags: KEYEVENTF_UNICODE });
        send({ wScan: unit, dwFlags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP });
      }
    },
  };
}
