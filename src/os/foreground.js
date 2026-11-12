// Polls the foreground window's process (exe name + elevation) via user32/kernel32/advapi32.
import { EventEmitter } from 'node:events';
import path from 'node:path';
import koffi from 'koffi';

const PROCESS_QUERY_LIMITED_INFORMATION = 0x1000;
const TOKEN_QUERY = 0x0008;
const TokenElevation = 20;

let api = null;
function load() {
  if (api) return api;
  const user32 = koffi.load('user32.dll');
  const kernel32 = koffi.load('kernel32.dll');
  const advapi32 = koffi.load('advapi32.dll');
  api = {
    GetForegroundWindow: user32.func('__stdcall', 'GetForegroundWindow', 'void *', []),
    GetWindowThreadProcessId: user32.func('__stdcall', 'GetWindowThreadProcessId', 'uint32', ['void *', koffi.out(koffi.pointer('uint32'))]),
    OpenProcess: kernel32.func('__stdcall', 'OpenProcess', 'void *', ['uint32', 'int', 'uint32']),
    CloseHandle: kernel32.func('__stdcall', 'CloseHandle', 'int', ['void *']),
    GetCurrentProcess: kernel32.func('__stdcall', 'GetCurrentProcess', 'void *', []),
    QueryFullProcessImageNameW: kernel32.func('__stdcall', 'QueryFullProcessImageNameW', 'int',
      ['void *', 'uint32', koffi.out(koffi.pointer('char16_t')), koffi.inout(koffi.pointer('uint32'))]),
    OpenProcessToken: advapi32.func('__stdcall', 'OpenProcessToken', 'int', ['void *', 'uint32', koffi.out(koffi.pointer('void *'))]),
    GetTokenInformation: advapi32.func('__stdcall', 'GetTokenInformation', 'int',
      ['void *', 'int', koffi.out(koffi.pointer('uint32')), 'uint32', koffi.out(koffi.pointer('uint32'))]),
  };
  return api;
}

function tokenElevated(hProcess) {
  const a = load();
  const tok = [null];
  if (!a.OpenProcessToken(hProcess, TOKEN_QUERY, tok)) return null;
  try {
    const val = [0];
    const len = [0];
    if (!a.GetTokenInformation(tok[0], TokenElevation, val, 4, len)) return null;
    return val[0] !== 0;
  } finally {
    a.CloseHandle(tok[0]);
  }
}

let selfElevated = null;
export function isSelfElevated() {
  if (selfElevated === null) selfElevated = tokenElevated(load().GetCurrentProcess()) === true;
  return selfElevated;
}

/** → { exe: string|null, elevated: boolean } for the current foreground window. */
export function queryForeground() {
  const a = load();
  const hwnd = a.GetForegroundWindow();
  if (!hwnd) return { exe: null, elevated: false };
  const pid = [0];
  a.GetWindowThreadProcessId(hwnd, pid);
  if (!pid[0]) return { exe: null, elevated: false };
  const h = a.OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid[0]);
  if (!h) return { exe: null, elevated: !isSelfElevated() };
  try {
    const buf = Buffer.alloc(1024 * 2);
    const size = [1024];
    const exe = a.QueryFullProcessImageNameW(h, 0, buf, size) ? path.win32.basename(buf.toString('utf16le', 0, size[0] * 2)) : null;
    const el = tokenElevated(h);
    const elevated = isSelfElevated() ? false : el === null ? true : el;
    return { exe, elevated };
  } finally {
    a.CloseHandle(h);
  }
}

export function createForegroundWatcher({ intervalMs = 500, query = queryForeground } = {}) {
  const em = new EventEmitter();
  let timer = null;
  let last = { exe: undefined, elevated: undefined };
  const tick = () => {
    let cur;
    try { cur = query(); } catch { return; }
    if (cur.exe !== last.exe || cur.elevated !== last.elevated) {
      last = cur;
      em.emit('change', cur);
    }
  };
  em.start = () => { if (!timer) { tick(); timer = setInterval(tick, intervalMs); } };
  em.stop = () => { clearInterval(timer); timer = null; };
  em.current = () => last;
  return em;
}
