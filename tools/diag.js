// Hardware diagnostics for the D6.
//   npm run diag                  numbers 1–15 on the keys + log presses
//   npm run diag -- --no-report-id  write without the leading report-ID byte
//   npm run diag -- --type        keyboard output test (focus Notepad within 3 s)
import HID from 'node-hid';
import { DeckDevice } from '../src/deck/device.js';
import { renderKeyJpeg } from '../src/deck/render.js';
import { VENDOR_ID, PRODUCT_ID } from '../src/deck/protocol.js';

const args = process.argv.slice(2);

if (args.includes('--type')) {
  const { createKeyboard } = await import('../src/input/keyboard.js');
  const kb = createKeyboard();
  console.log('Za 3 s píšu do okna v popředí — klikni do Poznámkového bloku.');
  await new Promise((r) => setTimeout(r, 3000));
  const tap = (code) => { kb.down(code); kb.up(code); };
  kb.down('ShiftLeft'); tap('KeyH'); kb.up('ShiftLeft'); console.log('Shift+KeyH → očekávej H');
  tap('KeyY'); console.log('KeyY → na české QWERTZ očekávej z');
  tap('Digit1'); console.log('Digit1 → na české klávesnici očekávej +');
  tap('Space');
  kb.typeText('ěščř'); console.log('text ěščř');
  tap('F1'); console.log('F1 (v Poznámkovém bloku otevře nápovědu, můžeš zavřít)');
  process.exit(0);
}

const list = (await HID.devicesAsync()).filter((d) => d.vendorId === VENDOR_ID && d.productId === PRODUCT_ID);
console.log('Rozhraní D6:');
for (const d of list) console.log(`  usagePage=0x${d.usagePage.toString(16)} usage=${d.usage} interface=${d.interface} ${d.path}`);

const deck = new DeckDevice({ reportIdPrefix: !args.includes('--no-report-id'), brightness: 80 });
deck.on('error', (e) => console.error('chyba:', e.message));
deck.on('connected', async () => {
  console.log('Připojeno, nahrávám čísla 1–15…');
  const images = new Map();
  for (let k = 1; k <= 15; k++) images.set(k, await renderKeyJpeg({ label: String(k), bg: k <= 5 ? '#1f5f3a' : k <= 10 ? '#1f3a5f' : '#5f1f3a' }));
  deck.setImages(images);
});
deck.on('disconnected', () => console.log('Odpojeno, čekám…'));
deck.on('raw', ({ deviceId, pressed }) => {
  const key = deck.keyMap[deviceId - 1];
  console.log(`raw id=0x${deviceId.toString(16).padStart(2, '0')} ${pressed ? 'down' : 'up  '} → logická klávesa ${key}`);
});
deck.start();
console.log('Ctrl+C ukončí.');
process.on('SIGINT', async () => { await deck.stop(); process.exit(0); });
