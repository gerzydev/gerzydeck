// Running-process listing (for the profile picker and the vendor-software conflict check).
import { execFile } from 'node:child_process';

const VENDOR = /fifine|streamdock|ampligame|hotspot/i;

export function parseTasklistCsv(text) {
  const names = new Set();
  for (const line of text.split(/\r?\n/)) {
    const m = /^"([^"]+)"/.exec(line);
    if (m) names.add(m[1].toLowerCase());
  }
  return [...names].sort();
}

export function listProcessNames() {
  return new Promise((resolve) => {
    execFile('tasklist', ['/fo', 'csv', '/nh'], { windowsHide: true, encoding: 'latin1', maxBuffer: 8 * 1024 * 1024 },
      (err, stdout) => resolve(err ? [] : parseTasklistCsv(stdout)));
  });
}

export const findConflicts = (names) => names.filter((n) => VENDOR.test(n));
