// One-click presets for the key editor: fill action + label; templates with an `icon` also set a built-in image.
export const TEMPLATES = [
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
