// One-click presets for the key editor: fill action + label; templates with an `icon` also set a built-in image.
const DISCORD_HINT = (what) =>
  `V Discordu: Uživatelská nastavení → Klávesové zkratky → Přidat klávesovou zkratku → ${what}, ` +
  'klikni do pole pro zkratku a zmáčkni toto tlačítko na decku.';

export const TEMPLATES = [
  { group: 'Discord', name: 'Discord – ztlumit mikrofon', label: '', icon: 'discord-mute', action: { type: 'keys', codes: ['ControlLeft', 'ShiftLeft', 'F13'] }, hint: DISCORD_HINT('Ztlumit') },
  { group: 'Discord', name: 'Discord – ohlušit', label: '', icon: 'discord-deafen', action: { type: 'keys', codes: ['ControlLeft', 'ShiftLeft', 'F14'] }, hint: DISCORD_HINT('Ohlušit') },
  { group: 'Windows', name: 'Plocha', label: 'Plocha', action: { type: 'keys', codes: ['MetaLeft', 'KeyD'] } },
  { group: 'Windows', name: 'Výstřižek obrazovky', label: 'Výstřižek', action: { type: 'keys', codes: ['MetaLeft', 'ShiftLeft', 'KeyS'] } },
  { group: 'Windows', name: 'Průzkumník', label: 'Soubory', action: { type: 'keys', codes: ['MetaLeft', 'KeyE'] } },
  { group: 'Windows', name: 'Správce úloh', label: 'Úlohy', action: { type: 'keys', codes: ['ControlLeft', 'ShiftLeft', 'Escape'] } },
  { group: 'Windows', name: 'Zamknout PC', label: 'Zamknout', action: { type: 'system', command: 'lock' } },
  { group: 'Média', name: 'Přehrát / pauza', label: '⏯', action: { type: 'media', command: 'playpause' } },
  { group: 'Média', name: 'Hlasitost +', label: 'Vol +', action: { type: 'media', command: 'volup' } },
  { group: 'Média', name: 'Hlasitost −', label: 'Vol −', action: { type: 'media', command: 'voldown' } },
  { group: 'Média', name: 'Ztlumit zvuk', label: '', icon: 'volume-mute', action: { type: 'media', command: 'mute' } },
];

export const sameAction = (a, b) => JSON.stringify(a) === JSON.stringify(b);
