// Built-in key icons, drawn for Gerzy's deck (original artwork, no third-party logos).
// Black background so they blend into the deck's keys; coloured glyph on a 24×24 grid.
// The editor rasterises them to a 200×200 PNG on demand.
import { t } from './i18n.js';

const BG = '#000000';
const BLURPLE = '#7B85F7'; // lifted Discord blurple, reads better on a black LCD
const RED = '#F04A4D';
const GREEN = '#43C46A';
const WHITE = '#EEF0F2';

const tile = (color, glyph) =>
  `<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200" viewBox="0 0 24 24">` +
  `<rect width="24" height="24" fill="${BG}"/>` +
  `<g transform="translate(3.3 3.3) scale(0.725)" fill="none" stroke="${color}" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round">${glyph(color)}</g></svg>`;

// red slash with a black gap so it reads on top of the glyph
const SLASH = `<path d="M3 3l18 18" stroke="${BG}" stroke-width="5.2"/><path d="M3 3l18 18" stroke="${RED}" stroke-width="2.3"/>`;

const mic = () => '<rect x="9" y="2" width="6" height="11.5" rx="3"/><path d="M5 10.5a7 7 0 0 0 14 0"/><path d="M12 17.5V21.5M8.5 21.5h7"/>';
const headphones = (c) => `<path d="M3.5 15v-3a8.5 8.5 0 0 1 17 0v3"/><rect x="2.5" y="14" width="5" height="7.5" rx="2" fill="${c}"/><rect x="16.5" y="14" width="5" height="7.5" rx="2" fill="${c}"/>`;
const speaker = (c) => `<path d="M3.5 9.5h3.5L11.5 5.5v13l-4.5-4H3.5z" fill="${c}"/>`;
const speakerWaves = (c) => `${speaker(c)}<path d="M15 9a4.2 4.2 0 0 1 0 6M17.8 6.5a8 8 0 0 1 0 11"/>`;

export const ICONS = [
  { id: 'discord-mute', name: t("Discord – microphone off"), svg: tile(BLURPLE, (c) => mic(c) + SLASH) },
  { id: 'discord-mic', name: t("Discord – microphone"), svg: tile(BLURPLE, mic) },
  { id: 'discord-deafen', name: t("Discord – deafen"), svg: tile(BLURPLE, (c) => headphones(c) + SLASH) },
  { id: 'discord-headphones', name: t("Discord – headphones"), svg: tile(BLURPLE, headphones) },
  { id: 'mic-live', name: t("Microphone on"), svg: tile(GREEN, mic) },
  { id: 'volume-mute', name: t("Mute"), svg: tile(WHITE, (c) => speaker(c) + SLASH) },
  { id: 'volume', name: t("Sound"), svg: tile(WHITE, speakerWaves) },
];

export const iconById = (id) => ICONS.find((i) => i.id === id);

/** SVG → PNG bytes (200×200) using the webview's own renderer. */
export async function rasterizeIcon(icon, size = 200) {
  const img = new Image();
  img.src = 'data:image/svg+xml;charset=utf-8,' + encodeURIComponent(icon.svg);
  await img.decode();
  const canvas = document.createElement('canvas');
  canvas.width = canvas.height = size;
  canvas.getContext('2d').drawImage(img, 0, 0, size, size);
  const blob = await new Promise((resolve) => canvas.toBlob(resolve, 'image/png'));
  return new Uint8Array(await blob.arrayBuffer());
}
