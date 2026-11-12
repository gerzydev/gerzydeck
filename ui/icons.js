// Built-in key icons, drawn for Gerzy's deck (original artwork, no third-party logos).
// Each is an SVG on a 24×24 grid; the editor rasterises it to a 200×200 PNG on demand.

const BLURPLE = '#5865F2';
const RED = '#ED4245';
const GREEN = '#3BA55D';

const tile = (bg, glyph) =>
  `<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200" viewBox="0 0 24 24">` +
  `<rect width="24" height="24" fill="${bg}"/>` +
  `<g transform="translate(4.2 4.2) scale(0.65)" fill="none" stroke="#fff" stroke-width="2.1" stroke-linecap="round" stroke-linejoin="round">${glyph}</g></svg>`;

// slash with a gap in the background colour so it reads on top of the glyph
const slash = (bg) => `<path d="M3 3l18 18" stroke="${bg}" stroke-width="5.5"/><path d="M3 3l18 18" stroke="${RED}" stroke-width="2.6"/>`;

const MIC = '<rect x="9" y="2" width="6" height="11.5" rx="3"/><path d="M5 10.5a7 7 0 0 0 14 0"/><path d="M12 17.5V21.5M8.5 21.5h7"/>';
const HEADPHONES = '<path d="M3.5 15v-3a8.5 8.5 0 0 1 17 0v3"/><rect x="2.5" y="14" width="5" height="7.5" rx="2" fill="#fff"/><rect x="16.5" y="14" width="5" height="7.5" rx="2" fill="#fff"/>';
const SPEAKER = '<path d="M4 9.5h3.5L12 5.5v13l-4.5-4H4z" fill="#fff"/>';

export const ICONS = [
  { id: 'discord-mute', name: 'Discord – mikrofon vypnutý', svg: tile(BLURPLE, MIC + slash(BLURPLE)) },
  { id: 'discord-mic', name: 'Discord – mikrofon', svg: tile(BLURPLE, MIC) },
  { id: 'discord-deafen', name: 'Discord – ohlušit', svg: tile(BLURPLE, HEADPHONES + slash(BLURPLE)) },
  { id: 'discord-headphones', name: 'Discord – sluchátka', svg: tile(BLURPLE, HEADPHONES) },
  { id: 'mic-live', name: 'Mikrofon zapnutý', svg: tile(GREEN, MIC) },
  { id: 'volume-mute', name: 'Ztlumit zvuk', svg: tile('#2b2d31', SPEAKER + slash('#2b2d31')) },
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
