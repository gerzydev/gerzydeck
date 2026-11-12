import { test } from 'node:test';
import assert from 'node:assert/strict';
import { findConflicts, parseTasklistCsv } from '../src/os/processes.js';

test('findConflicts picks vendor software', () => {
  assert.deepEqual(
    findConflicts(['explorer.exe', 'fifine control deck.exe', 'streamdock.exe', 'wow.exe']),
    ['fifine control deck.exe', 'streamdock.exe'],
  );
});

test('parseTasklistCsv: unique, lower-case, sorted; tolerates odd lines', () => {
  const csv = '"Wow.exe","123","Console","1","100 K"\r\n"wow.exe","5","Console","1","1 K"\r\n"Discord.exe","9","Console","1","1 K"\r\n\r\ngarbage\r\n';
  assert.deepEqual(parseTasklistCsv(csv), ['discord.exe', 'wow.exe']);
});
