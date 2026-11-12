// Wire protocol of the Fifine AmpliGame D6 (VID 0x3142, PID 0x0060, firmware V3).
// Source: docs/reference/fifine-ampligame-d6 (reverse engineered, MIT).

export const PACKET_SIZE = 1024;
export const NUM_KEYS = 15;
export const VENDOR_ID = 0x3142;
export const PRODUCT_ID = 0x0060;
export const USAGE_PAGE = 0xffa0;

const HEADER = Buffer.from('CRT\0\0', 'latin1');

/** Builds a zero-padded 1024-byte command: "CRT\0\0" + body. */
function command(body) {
  const pkt = Buffer.alloc(PACKET_SIZE);
  HEADER.copy(pkt, 0);
  Buffer.from(body).copy(pkt, HEADER.length);
  return pkt;
}

const ascii = (s) => [...Buffer.from(s, 'latin1')];

export const connectPacket = () => command(ascii('CONNECT\0\0\0'));
export const blefPacket = () => command(ascii('BLEF\0'));
export const modPacket = () => command([...ascii('MOD\0\0'), 1]);
export const wakePacket = () => command([...ascii('DIS\0\0'), 1]);
export const clearPacket = () => command([...ascii('CLE\0\0\0'), 0xff]);
export const stopPacket = () => command(ascii('STP\0\0'));
export const pixmapEndPacket = () => command(ascii('PIXMAPEND'));
export const sleepPacket = () => command(ascii('SLP\0\0'));

export function brightnessPacket(level) {
  const v = Math.max(0, Math.min(100, Math.round(Number(level) || 0)));
  return command([...ascii('LIG\0\0'), v]);
}

/** Logical key (1 = top-left, row-major) → LCD slot. Rows are mounted bottom-up. */
export function keyToSlot(key) {
  if (key >= 1 && key <= 5) return key + 10;
  if (key >= 11 && key <= 15) return key - 10;
  return key;
}

export function imageUploadPackets(key, jpeg) {
  if (!(key >= 1 && key <= NUM_KEYS)) throw new RangeError(`key ${key} out of range`);
  if (jpeg.length > 0xffff) throw new RangeError('jpeg too large');
  const header = command([...ascii('BAT\0\0'), (jpeg.length >> 8) & 0xff, jpeg.length & 0xff, keyToSlot(key)]);
  const chunks = [];
  for (let off = 0; off < jpeg.length; off += PACKET_SIZE) {
    const chunk = Buffer.alloc(PACKET_SIZE);
    jpeg.copy(chunk, 0, off, Math.min(off + PACKET_SIZE, jpeg.length));
    chunks.push(chunk);
  }
  return [header, ...chunks, stopPacket()];
}

export const initPackets = (brightness) => [
  connectPacket(), blefPacket(), modPacket(), wakePacket(), brightnessPacket(brightness), clearPacket(), stopPacket(),
];

export const BATCH_END = () => [pixmapEndPacket(), stopPacket()];

/** Windows hidapi needs the report-ID byte (0) in front of the payload. */
export const withReportId = (pkt) => Buffer.concat([Buffer.from([0]), pkt]);

/** index = device id - 1, value = logical key */
export const DEFAULT_KEY_MAP = Object.freeze(Array.from({ length: NUM_KEYS }, (_, i) => i + 1));

export function parseInputReport(buf, keyMap = DEFAULT_KEY_MAP) {
  if (!buf || buf.length < 11) return null;
  if (buf[0] === 0 && buf.length >= 12 && buf.toString('latin1', 1, 4) === 'ACK') buf = buf.subarray(1);
  if (buf.toString('latin1', 0, 3) !== 'ACK' || buf.toString('latin1', 5, 7) !== 'OK') return null;
  const id = buf[9];
  if (id === 0 || id === 0xff) return { type: 'ack' };
  if (id < 1 || id > NUM_KEYS) return null;
  const key = (keyMap ?? DEFAULT_KEY_MAP)[id - 1];
  return { type: 'key', deviceId: id, key, pressed: buf[10] === 1 };
}
