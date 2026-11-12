// One-click presets for the key editor: fill action + label; templates with an `icon` also set a built-in image.
import { t } from './i18n.js';
export const TEMPLATES = [
  { group: 'Windows', name: t("Desktop"), label: t("Desktop"), action: { type: 'keys', codes: ['MetaLeft', 'KeyD'] } },
  { group: 'Windows', name: t("Screen snip"), label: t("Snip"), action: { type: 'keys', codes: ['MetaLeft', 'ShiftLeft', 'KeyS'] } },
  { group: 'Windows', name: t("File Explorer"), label: t("Files"), action: { type: 'keys', codes: ['MetaLeft', 'KeyE'] } },
  { group: 'Windows', name: t("Task Manager"), label: t("Tasks"), action: { type: 'keys', codes: ['ControlLeft', 'ShiftLeft', 'Escape'] } },
  { group: 'Windows', name: t("Lock PC"), label: 'Zamknout', action: { type: 'system', command: 'lock' } },
  { group: t("Media"), name: t("Play / pause"), label: '⏯', action: { type: 'media', command: 'playpause' } },
  { group: t("Media"), name: t("Volume +"), label: 'Vol +', action: { type: 'media', command: 'volup' } },
  { group: t("Media"), name: t("Volume −"), label: 'Vol −', action: { type: 'media', command: 'voldown' } },
  { group: t("Media"), name: t("Mute"), label: '', icon: 'volume-mute', action: { type: 'media', command: 'mute' } },
];

export const sameAction = (a, b) => JSON.stringify(a) === JSON.stringify(b);
