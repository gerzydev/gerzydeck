"""LCD Framebuffer Engine for Fifine AmpliGame Stream Deck (D6 / D6-Pro).

Implements the proprietary 512-byte 'CRT' USB output protocol to stream
JPEG frames, control backlight brightness, and wake the hardware from cold standby.
Works 100% standalone with zero binary dependencies.
"""

import os
import struct
import subprocess
import tempfile
from pathlib import Path
from typing import Dict, List, Optional, Tuple, Union, Any

# Packet Protocol Constants
PACKET_SIZE = 1024
HEADER_CRT = b"CRT\x00\x00"

# Proprietary Output Commands (StreamDock / D6 hardware protocol)
CMD_CONNECT = HEADER_CRT + b"CONNECT\x00\x00\x00" + b"\x00" * (PACKET_SIZE - 15)
CMD_CONN = CMD_CONNECT
CMD_BLEF = HEADER_CRT + b"BLEF\x00" + b"\x00" * (PACKET_SIZE - 10)
CMD_MOD = HEADER_CRT + b"MOD\x00\x00\x01" + b"\x00" * (PACKET_SIZE - 11)
CMD_DIS = HEADER_CRT + b"DIS\x00\x00\x01" + b"\x00" * (PACKET_SIZE - 11)       # Wake screen (0x01 at wire offset 10)
CMD_STP = HEADER_CRT + b"STP\x00\x00" + b"\x00" * (PACKET_SIZE - 10)       # Commit / refresh framebuffer
CMD_CLE = HEADER_CRT + b"CLE\x00\x00\x00\xff" + b"\x00" * (PACKET_SIZE - 11) # Clear all LCD screens
CMD_SLP = HEADER_CRT + b"SLP\x00\x00" + b"\x00" * (PACKET_SIZE - 10)       # Sleep screen
CMD_PIXMAPEND = HEADER_CRT + b"PIXMAPEND" + b"\x00" * (PACKET_SIZE - 14)    # Master upload finish token
CMD_ULEND = CMD_STP        # Alias for backward compatibility

DEFAULT_KEY_WIDTH = 100
DEFAULT_KEY_HEIGHT = 100
NUM_KEYS = 15

# Minimal 8x8 ASCII font dictionary for zero-dependency tile rendering
FONT_8X8: Dict[str, List[int]] = {
    " ": [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
    "0": [0x3C, 0x66, 0x6E, 0x76, 0x66, 0x66, 0x3C, 0x00],
    "1": [0x18, 0x38, 0x18, 0x18, 0x18, 0x18, 0x3C, 0x00],
    "2": [0x3C, 0x66, 0x06, 0x0C, 0x18, 0x30, 0x7E, 0x00],
    "3": [0x3C, 0x66, 0x06, 0x1C, 0x06, 0x66, 0x3C, 0x00],
    "4": [0x0C, 0x1C, 0x34, 0x64, 0x7E, 0x04, 0x0C, 0x00],
    "5": [0x7E, 0x60, 0x7C, 0x06, 0x06, 0x66, 0x3C, 0x00],
    "6": [0x1C, 0x30, 0x60, 0x7C, 0x66, 0x66, 0x3C, 0x00],
    "7": [0x7E, 0x06, 0x0C, 0x18, 0x30, 0x30, 0x30, 0x00],
    "8": [0x3C, 0x66, 0x66, 0x3C, 0x66, 0x66, 0x3C, 0x00],
    "9": [0x3C, 0x66, 0x66, 0x3E, 0x06, 0x0C, 0x38, 0x00],
    "A": [0x18, 0x3C, 0x66, 0x66, 0x7E, 0x66, 0x66, 0x00],
    "B": [0x7C, 0x66, 0x66, 0x7C, 0x66, 0x66, 0x7C, 0x00],
    "C": [0x3C, 0x66, 0x60, 0x60, 0x60, 0x66, 0x3C, 0x00],
    "D": [0x78, 0x6C, 0x66, 0x66, 0x66, 0x6C, 0x78, 0x00],
    "E": [0x7E, 0x60, 0x60, 0x78, 0x60, 0x60, 0x7E, 0x00],
    "F": [0x7E, 0x60, 0x60, 0x78, 0x60, 0x60, 0x60, 0x00],
    "G": [0x3C, 0x66, 0x60, 0x6E, 0x66, 0x66, 0x3A, 0x00],
    "H": [0x66, 0x66, 0x66, 0x7E, 0x66, 0x66, 0x66, 0x00],
    "I": [0x3C, 0x18, 0x18, 0x18, 0x18, 0x18, 0x3C, 0x00],
    "J": [0x1E, 0x0C, 0x0C, 0x0C, 0x0C, 0x6C, 0x38, 0x00],
    "K": [0x66, 0x6C, 0x78, 0x70, 0x78, 0x6C, 0x66, 0x00],
    "L": [0x60, 0x60, 0x60, 0x60, 0x60, 0x60, 0x7E, 0x00],
    "M": [0x63, 0x77, 0x7F, 0x6B, 0x63, 0x63, 0x63, 0x00],
    "N": [0x66, 0x76, 0x7E, 0x7E, 0x6E, 0x66, 0x66, 0x00],
    "O": [0x3C, 0x66, 0x66, 0x66, 0x66, 0x66, 0x3C, 0x00],
    "P": [0x7C, 0x66, 0x66, 0x7C, 0x60, 0x60, 0x60, 0x00],
    "Q": [0x3C, 0x66, 0x66, 0x66, 0x6A, 0x3C, 0x0E, 0x00],
    "R": [0x7C, 0x66, 0x66, 0x7C, 0x78, 0x6C, 0x66, 0x00],
    "S": [0x3C, 0x66, 0x60, 0x3C, 0x06, 0x66, 0x3C, 0x00],
    "T": [0x7E, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x00],
    "U": [0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x3C, 0x00],
    "V": [0x66, 0x66, 0x66, 0x66, 0x66, 0x3C, 0x18, 0x00],
    "W": [0x63, 0x63, 0x63, 0x6B, 0x7F, 0x77, 0x63, 0x00],
    "X": [0x66, 0x66, 0x3C, 0x18, 0x3C, 0x66, 0x66, 0x00],
    "Y": [0x66, 0x66, 0x66, 0x3C, 0x18, 0x18, 0x18, 0x00],
    "Z": [0x7E, 0x06, 0x0C, 0x18, 0x30, 0x60, 0x7E, 0x00],
    "-": [0x00, 0x00, 0x00, 0x7E, 0x00, 0x00, 0x00, 0x00],
    "_": [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x7E, 0x00],
    "+": [0x00, 0x18, 0x18, 0x7E, 0x18, 0x18, 0x00, 0x00],
    "=": [0x00, 0x7E, 0x00, 0x7E, 0x00, 0x00, 0x00, 0x00],
    "/": [0x02, 0x06, 0x0C, 0x18, 0x30, 0x60, 0x40, 0x00],
    ":": [0x00, 0x18, 0x18, 0x00, 0x18, 0x18, 0x00, 0x00],
    ".": [0x00, 0x00, 0x00, 0x00, 0x00, 0x18, 0x18, 0x00],
    ",": [0x00, 0x00, 0x00, 0x00, 0x00, 0x18, 0x18, 0x30],
    "!": [0x18, 0x18, 0x18, 0x18, 0x18, 0x00, 0x18, 0x00],
    "?": [0x3C, 0x66, 0x06, 0x0C, 0x18, 0x00, 0x18, 0x00],
    "#": [0x24, 0x24, 0x7E, 0x24, 0x7E, 0x24, 0x24, 0x00],
    ">": [0x60, 0x30, 0x18, 0x0C, 0x18, 0x30, 0x60, 0x00],
    "<": [0x06, 0x0C, 0x18, 0x30, 0x18, 0x0C, 0x06, 0x00],
}


def make_brightness_packet(level: int) -> bytes:
    """Builds a 512-byte CRT LIG brightness packet (level: 0..100)."""
    clamped = max(0, min(100, int(level)))
    # Bytes 0-4: "CRT\0\0", Bytes 5-9: "LIG\0\0", Byte 10: level
    return HEADER_CRT + b"LIG\x00\x00" + bytes([clamped]) + b"\x00" * (PACKET_SIZE - 11)


def logical_to_hardware_slot(key_id: int) -> int:
    """Translates a logical key ID (1..15, top row 1..5, bottom row 11..15)
    to the physical hardware LCD panel slot ID.

    Physical LCD panel slot geometry:
      Top Row (Logical 1..5):     Hardware LCD slots 11..15
      Middle Row (Logical 6..10): Hardware LCD slots  6..10
      Bottom Row (Logical 11..15): Hardware LCD slots  1..5
    """
    if 1 <= key_id <= 5:
        return key_id + 10
    elif 11 <= key_id <= 15:
        return key_id - 10
    return key_id


def make_image_header(key_id: int, image_byte_len: int) -> bytes:
    """Builds a 1024-byte CRT BAT image allocation header.

    Bytes 0-4: "CRT\0\0"
    Bytes 5-9: "BAT\0\0"
    Bytes 10-11: big-endian uint16 image byte length
    Byte 12: hardware slot index (1..15)
    Remaining: null padded
    """
    if not (1 <= key_id <= NUM_KEYS):
        raise ValueError(f"Key ID must be between 1 and {NUM_KEYS}, got {key_id}")
    hw_slot = logical_to_hardware_slot(key_id)
    return (
        HEADER_CRT
        + b"BAT\x00\x00"
        + struct.pack(">H", image_byte_len)
        + bytes([hw_slot])
        + b"\x00" * (PACKET_SIZE - 13)
    )


def slice_image_chunks(jpeg_bytes: bytes) -> List[bytes]:
    """Slices JPEG bytes into 1024-byte packets, zero-padded if necessary."""
    packets = []
    total_len = len(jpeg_bytes)
    for offset in range(0, total_len, PACKET_SIZE):
        chunk = jpeg_bytes[offset : offset + PACKET_SIZE]
        if len(chunk) < PACKET_SIZE:
            chunk = chunk + b"\x00" * (PACKET_SIZE - len(chunk))
        packets.append(chunk)
    return packets


def parse_color(color: Union[str, Tuple[int, int, int], List[int]]) -> Tuple[int, int, int]:
    """Parses hex '#RRGGBB' or RGB tuple into (r, g, b) tuple."""
    if isinstance(color, str):
        c = color.strip().lstrip("#")
        if len(c) == 3:
            c = "".join([ch * 2 for ch in c])
        if len(c) == 6:
            return (int(c[0:2], 16), int(c[2:4], 16), int(c[4:6], 16))
        raise ValueError(f"Invalid hex color string: {color}")
    if isinstance(color, (tuple, list)) and len(color) >= 3:
        return (int(color[0]), int(color[1]), int(color[2]))
    return (255, 255, 255)


def draw_text_onto_buffer(
    pixels: bytearray,
    width: int,
    height: int,
    text: str,
    start_x: int,
    start_y: int,
    scale: int = 2,
    color: Tuple[int, int, int] = (255, 255, 255),
):
    """Draws string characters onto an RGB pixel buffer using the built-in 8x8 font."""
    cur_x = start_x
    for char in text:
        bitmap = FONT_8X8.get(char.upper(), FONT_8X8[" "])
        for row_idx, row_byte in enumerate(bitmap):
            for col_idx in range(8):
                if (row_byte >> (7 - col_idx)) & 1:
                    for sy in range(scale):
                        for sx in range(scale):
                            px = cur_x + col_idx * scale + sx
                            py = start_y + row_idx * scale + sy
                            if 0 <= px < width and 0 <= py < height:
                                idx = (py * width + px) * 3
                                pixels[idx] = color[0]
                                pixels[idx + 1] = color[1]
                                pixels[idx + 2] = color[2]
        cur_x += 8 * scale


def rotate_buffer_180(pixels: bytearray, width: int, height: int) -> bytearray:
    """Rotates a 24-bit RGB pixel buffer 180° for physically inverted LCD panels."""
    out = bytearray(len(pixels))
    for y in range(height):
        for x in range(width):
            src_idx = (y * width + x) * 3
            dst_idx = ((height - 1 - y) * width + (width - 1 - x)) * 3
            out[dst_idx : dst_idx + 3] = pixels[src_idx : src_idx + 3]
    return out


def clean_baseline_jpeg(data: bytes) -> bytes:
    """Strips 0xFFDD (DRI) and APP metadata segments (0xFFE1..0xFFEF), preserving only baseline markers.

    Preserves only:
    - 0xFFD8: SOI (Start of Image)
    - 0xFFDB: DQT (Define Quantization Table)
    - 0xFFC0: SOF0 (Start of Frame, Baseline DCT)
    - 0xFFC4: DHT (Define Huffman Table)
    - 0xFFDA: SOS (Start of Scan) through 0xFFD9 (EOI: End of Image)
    """
    if not data or not data.startswith(b"\xff\xd8"):
        return data

    out = bytearray(b"\xff\xd8")
    i = 2
    n = len(data)
    while i < n:
        if data[i] != 0xFF:
            i += 1
            continue
        # Skip padding 0xFF bytes
        while i < n and data[i] == 0xFF:
            i += 1
        if i >= n:
            break

        marker = data[i]
        i += 1

        if marker == 0xD9:  # EOI
            out.extend(b"\xff\xd9")
            break
        elif marker == 0xDA:  # SOS
            # Start of scan: write marker, length, and all remaining payload (scan data + EOI)
            if i + 2 > n:
                break
            length = (data[i] << 8) | data[i + 1]
            out.extend(bytes([0xFF, marker]))
            out.extend(data[i : i + length])
            i += length
            out.extend(data[i:])
            break
        elif marker in (0xDB, 0xC0, 0xC4, 0xE0):  # DQT, SOF0, DHT, APP0 (JFIF)
            if i + 2 > n:
                break
            length = (data[i] << 8) | data[i + 1]
            out.extend(bytes([0xFF, marker]))
            out.extend(data[i : i + length])
            i += length
        else:
            # Standalone markers without length: RST0..RST7 (0xD0..0xD7), TEM (0x01)
            if (0xD0 <= marker <= 0xD7) or marker == 0x01:
                continue
            # Stripped segments with 16-bit length (DRI 0xFFDD, APP 0xFFE0..0xFFEF, COM 0xFFFE, etc.)
            if i + 2 > n:
                break
            length = (data[i] << 8) | data[i + 1]
            i += length

    if not out.endswith(b"\xff\xd9"):
        out.extend(b"\xff\xd9")

    return bytes(out)


def rgb_buffer_to_jpeg(pixels: bytes, width: int, height: int, quality: int = 90) -> bytes:
    """Encodes raw 24-bit RGB bytes into pure baseline JPEG format with 180° rotation.

    Uses Pillow if installed, otherwise uses native macOS 'sips' with zero binary dependencies.
    """
    # 180° buffer rotation for inverted LCD hardware mounting
    rotated = rotate_buffer_180(bytearray(pixels), width, height)

    # 1. Try Pillow if available
    try:
        from PIL import Image
        import io
        img = Image.frombytes("RGB", (width, height), bytes(rotated))
        buf = io.BytesIO()
        img.save(buf, format="JPEG", quality=quality)
        return clean_baseline_jpeg(buf.getvalue())
    except ImportError:
        pass

    # 2. Try ffmpeg if available (pure baseline JPEG without restart markers)
    try:
        header = f"P6\n{width} {height}\n255\n".encode("ascii")
        proc = subprocess.run(
            ["ffmpeg", "-y", "-f", "image2pipe", "-vcodec", "ppm", "-i", "pipe:0", "-vframes", "1", "-q:v", "2", "-f", "image2", "pipe:1"],
            input=header + rotated,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            check=True,
        )
        return clean_baseline_jpeg(proc.stdout)
    except (subprocess.SubprocessError, FileNotFoundError):
        pass

    # 3. Use native macOS sips tool (built-in on every macOS system)
    with tempfile.NamedTemporaryFile(suffix=".ppm", delete=False) as f:
        ppm_path = f.name
        header = f"P6\n{width} {height}\n255\n".encode("ascii")
        f.write(header + rotated)

    jpg_path = ppm_path.replace(".ppm", ".jpg")
    try:
        res = subprocess.run(
            ["sips", "-s", "format", "jpeg", ppm_path, "--out", jpg_path],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=True,
        )
        with open(jpg_path, "rb") as f:
            jpeg_bytes = f.read()
        return clean_baseline_jpeg(jpeg_bytes)
    finally:
        if os.path.exists(ppm_path):
            os.remove(ppm_path)
        if os.path.exists(jpg_path):
            os.remove(jpg_path)


def generate_solid_color_jpeg(
    color: Union[str, Tuple[int, int, int]],
    width: int = DEFAULT_KEY_WIDTH,
    height: int = DEFAULT_KEY_HEIGHT,
) -> bytes:
    """Generates a solid color JPEG tile."""
    r, g, b = parse_color(color)
    pixels = bytes([r, g, b]) * (width * height)
    return rgb_buffer_to_jpeg(pixels, width, height)


def generate_key_tile(
    title: Optional[str] = None,
    subtitle: Optional[str] = None,
    number: Optional[int] = None,
    bg_color: Union[str, Tuple[int, int, int]] = (18, 24, 38),
    text_color: Union[str, Tuple[int, int, int]] = (255, 255, 255),
    border_color: Union[str, Tuple[int, int, int]] = (40, 80, 160),
    width: int = DEFAULT_KEY_WIDTH,
    height: int = DEFAULT_KEY_HEIGHT,
) -> bytes:
    """Generates a styled LCD key icon graphic in JPEG format in pure Python."""
    bg_r, bg_g, bg_b = parse_color(bg_color)
    bdr_r, bdr_g, bdr_b = parse_color(border_color)
    txt_r, txt_g, txt_b = parse_color(text_color)

    buf = bytearray(width * height * 3)
    # Render subtle dark background gradient with border
    for y in range(height):
        # subtle vertical gradient
        grad_r = min(255, max(0, bg_r + (y // 6)))
        grad_g = min(255, max(0, bg_g + (y // 5)))
        grad_b = min(255, max(0, bg_b + (y // 4)))

        for x in range(width):
            idx = (y * width + x) * 3
            # 1px border
            if x == 0 or x == width - 1 or y == 0 or y == height - 1:
                buf[idx] = bdr_r
                buf[idx + 1] = bdr_g
                buf[idx + 2] = bdr_b
            else:
                buf[idx] = grad_r
                buf[idx + 1] = grad_g
                buf[idx + 2] = grad_b

    # Draw elements
    if number is not None and not title:
        # Centered large number
        num_str = str(number)
        scale = 4 if len(num_str) <= 2 else 3
        char_w = 8 * scale
        total_w = len(num_str) * char_w
        start_x = max(2, (width - total_w) // 2)
        start_y = max(2, (height - (8 * scale)) // 2)
        draw_text_onto_buffer(buf, width, height, num_str, start_x, start_y, scale=scale, color=(txt_r, txt_g, txt_b))
    else:
        # Title + Subtitle layout
        if title:
            scale = 2 if len(title) <= 5 else 1
            char_w = 8 * scale
            total_w = len(title) * char_w
            start_x = max(2, (width - total_w) // 2)
            start_y = 24 if subtitle else max(2, (height - (8 * scale)) // 2)
            draw_text_onto_buffer(buf, width, height, title, start_x, start_y, scale=scale, color=(txt_r, txt_g, txt_b))

        if subtitle:
            scale = 1
            char_w = 8 * scale
            total_w = len(subtitle) * char_w
            start_x = max(2, (width - total_w) // 2)
            start_y = height - 28
            # Muted subtitle color
            sub_color = (max(0, txt_r - 60), max(0, txt_g - 60), max(0, txt_b - 60))
            draw_text_onto_buffer(buf, width, height, subtitle, start_x, start_y, scale=scale, color=sub_color)

    return rgb_buffer_to_jpeg(bytes(buf), width, height)


def convert_image_to_jpeg(
    image: Any,
    width: int = DEFAULT_KEY_WIDTH,
    height: int = DEFAULT_KEY_HEIGHT,
) -> bytes:
    """Converts various image types (JPEG bytes, file path, PIL Image, color) into standard JPEG bytes."""
    # 1. Raw bytes
    if isinstance(image, bytes):
        if image.startswith(b"\xff\xd8"):
            return clean_baseline_jpeg(image)
        # Maybe raw RGB? Check length
        if len(image) == width * height * 3:
            return rgb_buffer_to_jpeg(image, width, height)
        raise ValueError("Provided bytes do not start with JPEG SOI marker (0xFFD8) and are not matching RGB dimensions.")

    # 2. File Path
    if isinstance(image, (str, Path)):
        p = Path(image)
        if not p.is_file():
            # Check if it is a color string like "#00ff00"
            if isinstance(image, str) and (image.startswith("#") or image.startswith("rgb")):
                return generate_solid_color_jpeg(image, width, height)
            raise FileNotFoundError(f"Image file not found: {image}")

        # Try PIL first
        try:
            from PIL import Image
            with Image.open(p) as img:
                img_rgb = img.convert("RGB").resize((width, height), Image.Resampling.LANCZOS)
                return rgb_buffer_to_jpeg(img_rgb.tobytes(), width, height)
        except ImportError:
            pass

        # Try native macOS sips (resample to width x height, rotate 180°, and clean baseline JPEG)
        with tempfile.NamedTemporaryFile(suffix=".jpg", delete=False) as tmp_out:
            out_path = tmp_out.name

        try:
            subprocess.run(
                ["sips", "-s", "format", "jpeg", "-r", "180", "-z", str(height), str(width), str(p), "--out", out_path],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                check=True,
            )
            with open(out_path, "rb") as f:
                return clean_baseline_jpeg(f.read())
        finally:
            if os.path.exists(out_path):
                os.remove(out_path)

    # 3. PIL Image object
    if hasattr(image, "convert") and hasattr(image, "resize"):
        img_rgb = image.convert("RGB").resize((width, height))
        return rgb_buffer_to_jpeg(img_rgb.tobytes(), width, height)

    # 4. Color tuple/list
    if isinstance(image, (tuple, list)):
        return generate_solid_color_jpeg(image, width, height)

    raise TypeError(f"Unsupported image type: {type(image)}")


class FifineDisplay:
    """Manages LCD framebuffer streaming, brightness, and key icon uploads."""

    def __init__(self, driver: Any, width: int = DEFAULT_KEY_WIDTH, height: int = DEFAULT_KEY_HEIGHT):
        self.driver = driver
        self.width = width
        self.height = height
        self._brightness = 100
        self._framebuffer: Dict[int, bytes] = {}

        # Pre-populate default tiles
        self._init_default_tiles()

    def _init_default_tiles(self):
        """Generates crisp default key tiles for all 15 keys."""
        # Color palette for 15 keys
        palette = [
            ("#1e293b", "#38bdf8"), ("#1e293b", "#38bdf8"), ("#1e293b", "#38bdf8"), ("#1e293b", "#38bdf8"), ("#1e293b", "#38bdf8"),
            ("#1e293b", "#818cf8"), ("#1e293b", "#818cf8"), ("#1e293b", "#818cf8"), ("#1e293b", "#818cf8"), ("#1e293b", "#818cf8"),
            ("#1e293b", "#34d399"), ("#1e293b", "#34d399"), ("#1e293b", "#34d399"), ("#1e293b", "#34d399"), ("#1e293b", "#34d399"),
        ]
        for key_id in range(1, NUM_KEYS + 1):
            bg, bdr = palette[key_id - 1]
            self._framebuffer[key_id] = generate_key_tile(
                number=key_id,
                bg_color=bg,
                border_color=bdr,
                text_color="#ffffff",
                width=self.width,
                height=self.height,
            )

    def wake(self, initial_keys: Optional[Dict[int, Any]] = None, brightness: int = 100):
        """Executes the master cold-boot wake sequence for the Fifine Stream Deck.

        1. Query firmware version via 512-byte input report query.
        2. Send CRT CONN, CRT BLEF, CRT MOD.
        3. Set brightness (CRT LIG).
        4. Stream key frames (BAT header + chunks + ULEND per key).
        5. Master commit (CRT ULEND).
        """
        self._brightness = max(0, min(100, int(brightness)))

        if initial_keys:
            for k, img in initial_keys.items():
                if 1 <= k <= NUM_KEYS:
                    self._framebuffer[k] = convert_image_to_jpeg(img, self.width, self.height)

        # 1. Query firmware version
        fw = self.driver.get_firmware_version()
        if fw:
            print(f"[FifineDisplay] Detected MCU Firmware: {fw}", flush=True)

        packets = []
        # 2. Hardware initialization & screen wake sequence
        packets.append(CMD_CONNECT)
        packets.append(CMD_BLEF)
        packets.append(CMD_MOD)
        packets.append(CMD_DIS)
        packets.append(make_brightness_packet(self._brightness))
        packets.append(CMD_CLE)
        packets.append(CMD_STP)

        # 3. Stream 15 LCD key tiles
        for key_id in range(1, NUM_KEYS + 1):
            jpeg_bytes = self._framebuffer.get(key_id)
            if not jpeg_bytes:
                jpeg_bytes = generate_key_tile(number=key_id, width=self.width, height=self.height)
                self._framebuffer[key_id] = jpeg_bytes

            # Image header
            packets.append(make_image_header(key_id, len(jpeg_bytes)))
            # Image chunks
            packets.extend(slice_image_chunks(jpeg_bytes))
            # Commit key
            packets.append(CMD_STP)

        # 4. Master refresh
        packets.append(CMD_PIXMAPEND)
        packets.append(CMD_STP)

        # Send via paced write thread
        self.driver.send_reports(packets, blocking=True)
        print(f"[FifineDisplay] Master display wake sequence committed (15 keys, brightness {self._brightness}%).", flush=True)

    def set_brightness(self, level: int):
        """Sets display brightness (0..100%)."""
        self._brightness = max(0, min(100, int(level)))
        pkt_lig = make_brightness_packet(self._brightness)
        self.driver.send_reports([pkt_lig, CMD_STP], blocking=False)

    def set_key_image(self, key_id: int, image: Any, immediate: bool = True, blocking: bool = False):
        """Uploads an image to a specific LCD key."""
        if not (1 <= key_id <= NUM_KEYS):
            raise ValueError(f"key_id must be between 1 and {NUM_KEYS}, got {key_id}")

        jpeg_bytes = convert_image_to_jpeg(image, self.width, self.height)
        self._framebuffer[key_id] = jpeg_bytes

        if immediate and self.driver.is_connected:
            packets = [
                make_image_header(key_id, len(jpeg_bytes)),
                *slice_image_chunks(jpeg_bytes),
                CMD_STP,
                CMD_PIXMAPEND,
                CMD_STP,
            ]
            self.driver.send_reports(packets, blocking=blocking)

    def set_key_color(self, key_id: int, color: Union[str, Tuple[int, int, int]], immediate: bool = True, blocking: bool = False):
        """Sets a key tile to a solid color."""
        jpeg_bytes = generate_solid_color_jpeg(color, self.width, self.height)
        self.set_key_image(key_id, jpeg_bytes, immediate=immediate, blocking=blocking)

    def fill_color(self, color: Union[str, Tuple[int, int, int]], immediate: bool = True):
        """Fills all 15 key tiles with a solid color."""
        jpeg_bytes = generate_solid_color_jpeg(color, self.width, self.height)
        packets = []
        for key_id in range(1, NUM_KEYS + 1):
            self._framebuffer[key_id] = jpeg_bytes
            if immediate:
                packets.append(make_image_header(key_id, len(jpeg_bytes)))
                packets.extend(slice_image_chunks(jpeg_bytes))
                packets.append(CMD_STP)

        if immediate and self.driver.is_connected:
            packets.append(CMD_PIXMAPEND)
            packets.append(CMD_STP)
            self.driver.send_reports(packets, blocking=False)

    def clear(self, color: Union[str, Tuple[int, int, int]] = (0, 0, 0), immediate: bool = True):
        """Clears all 15 key tiles to black or specified color."""
        self.fill_color(color, immediate=immediate)

    def refresh_display(self):
        """Re-uploads all 15 keys in the active framebuffer."""
        packets = [CMD_CONNECT, CMD_BLEF, CMD_MOD, CMD_DIS, make_brightness_packet(self._brightness)]
        for key_id in range(1, NUM_KEYS + 1):
            jpeg_bytes = self._framebuffer.get(key_id)
            if not jpeg_bytes:
                jpeg_bytes = generate_key_tile(number=key_id, width=self.width, height=self.height)
                self._framebuffer[key_id] = jpeg_bytes

            packets.append(make_image_header(key_id, len(jpeg_bytes)))
            packets.extend(slice_image_chunks(jpeg_bytes))
            packets.append(CMD_STP)

        packets.append(CMD_PIXMAPEND)
        packets.append(CMD_STP)
        self.driver.send_reports(packets, blocking=True)
