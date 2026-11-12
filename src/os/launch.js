// Launching programs/URLs and system commands. Errors are logged, never thrown.
import { spawn } from 'node:child_process';
import koffi from 'koffi';

function detached(cmd, args, log) {
  try {
    const child = spawn(cmd, args, { detached: true, stdio: 'ignore', windowsHide: false });
    child.on('error', (err) => log.error?.(`spuštění ${cmd} selhalo:`, err.message));
    child.unref();
  } catch (err) {
    log.error?.(`spuštění ${cmd} selhalo:`, err.message);
  }
}

export function launchTarget(target, args = [], log = console) {
  if (/\.(exe|bat|cmd)$/i.test(target)) detached(target, args, log);
  else detached('explorer.exe', [target], log); // URLs, documents, folders → default handler
}

let lockWorkStation = null;
export function runSystem(command, log = console) {
  if (command === 'lock') {
    try {
      lockWorkStation ??= koffi.load('user32.dll').func('__stdcall', 'LockWorkStation', 'int', []);
      lockWorkStation();
    } catch (err) {
      log.error?.('zamknutí selhalo:', err.message);
    }
  } else if (command === 'sleep') {
    detached('rundll32.exe', ['powrprof.dll,SetSuspendState', '0,1,0'], log);
  }
}
