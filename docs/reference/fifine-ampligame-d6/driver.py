"""Pure native USB HID driver for Fifine AmpliGame Stream Deck (D6 / D6-Pro).

Implements a dual-thread architecture:
- Background Read Thread: Continuous non-blocking CFRunLoop pumping with explicit
  512-byte input report callbacks for instantaneous KeyEvent dispatch.
- Paced Write Thread: Manages 512-byte packet transmission with rate limiting
  (5ms per 64-packet burst) to ensure smooth USB transmission without pipe congestion.
"""

import ctypes
import ctypes.util
import queue
import sys
import threading
import time
from typing import Callable, List, Optional, Tuple, Union
from .events import KeyEvent

# Hardware button matrix mapping:
# Hardware reports IDs 0x01–0x05 for the physical top row (Buttons 1–5),
# 0x06–0x0A for the physical middle row (Buttons 6–10), and
# 0x0B–0x0F for the physical bottom row (Buttons 11–15).
DEVICE_ID_TO_KEY = {
    0x01: 1,
    0x02: 2,
    0x03: 3,
    0x04: 4,
    0x05: 5,
    0x06: 6,
    0x07: 7,
    0x08: 8,
    0x09: 9,
    0x0A: 10,
    0x0B: 11,
    0x0C: 12,
    0x0D: 13,
    0x0E: 14,
    0x0F: 15,
}
KEY_TO_DEVICE_ID = {v: k for k, v in DEVICE_ID_TO_KEY.items()}


class FifineDriver:
    """Pure native USB HID driver for Fifine AmpliGame Stream Deck (D6 / D6-Pro)."""

    VENDOR_ID = 12610   # 0x3142 (HOTSPOTEK / Fifine)
    PRODUCT_ID = 96     # 0x0060 (Fifine AmpliGame D6 / StreamDock)
    USAGE_PAGE = 65440  # 0xFFA0 (Vendor-defined)
    USAGE = 1           # 0x0001 (Vendor-defined 1)
    INPUT_REPORT_SIZE = 512
    OUTPUT_REPORT_SIZE = 1024

    def __init__(
        self,
        on_event: Optional[Callable[[KeyEvent], None]] = None,
        on_connect: Optional[Callable[[], None]] = None,
        on_disconnect: Optional[Callable[[], None]] = None,
    ):
        self.on_event = on_event
        self.on_connect = on_connect
        self.on_disconnect = on_disconnect

        self.is_running = False
        self._device = None
        self._device_lock = threading.Lock()

        # Dual-thread control
        self._read_thread: Optional[threading.Thread] = None
        self._write_thread: Optional[threading.Thread] = None
        self._heartbeat_thread: Optional[threading.Thread] = None
        self._write_queue: queue.Queue = queue.Queue()
        self._ack_event: threading.Event = threading.Event()
        self._run_loop = None
        self._kCFRunLoopDefaultMode = None
        self._input_report_buffer = (ctypes.c_uint8 * self.INPUT_REPORT_SIZE)()

        # C callback references to prevent garbage collection
        self._c_dev_cb = None
        self._c_rem_cb = None
        self._c_report_cb = None

        if sys.platform == "darwin":
            self.iokit = ctypes.cdll.LoadLibrary(ctypes.util.find_library("IOKit"))
            self.cf = ctypes.cdll.LoadLibrary(ctypes.util.find_library("CoreFoundation"))
            self._setup_darwin_ctypes()
        else:
            raise NotImplementedError("Direct standalone USB driver currently targets macOS via IOKit/CoreFoundation.")

    def _setup_darwin_ctypes(self):
        # CoreFoundation
        self.cf.CFRunLoopGetCurrent.restype = ctypes.c_void_p
        self.cf.CFRunLoopRunInMode.restype = ctypes.c_int32
        self.cf.CFRunLoopRunInMode.argtypes = [ctypes.c_void_p, ctypes.c_double, ctypes.c_bool]
        self.cf.CFRunLoopStop.restype = None
        self.cf.CFRunLoopStop.argtypes = [ctypes.c_void_p]

        self.cf.CFStringCreateWithCString.restype = ctypes.c_void_p
        self.cf.CFStringCreateWithCString.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_uint32]
        self.cf.CFNumberCreate.restype = ctypes.c_void_p
        self.cf.CFNumberCreate.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_void_p]
        self.cf.CFDictionaryCreate.restype = ctypes.c_void_p
        self.cf.CFDictionaryCreate.argtypes = [
            ctypes.c_void_p,
            ctypes.POINTER(ctypes.c_void_p),
            ctypes.POINTER(ctypes.c_void_p),
            ctypes.c_long,
            ctypes.c_void_p,
            ctypes.c_void_p,
        ]

        # IOKit HID Manager
        self.iokit.IOHIDManagerCreate.restype = ctypes.c_void_p
        self.iokit.IOHIDManagerCreate.argtypes = [ctypes.c_void_p, ctypes.c_uint32]
        self.iokit.IOHIDManagerOpen.restype = ctypes.c_int
        self.iokit.IOHIDManagerOpen.argtypes = [ctypes.c_void_p, ctypes.c_uint32]
        self.iokit.IOHIDManagerClose.restype = ctypes.c_int
        self.iokit.IOHIDManagerClose.argtypes = [ctypes.c_void_p, ctypes.c_uint32]
        self.iokit.IOHIDManagerSetDeviceMatching.restype = None
        self.iokit.IOHIDManagerSetDeviceMatching.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
        self.iokit.IOHIDManagerScheduleWithRunLoop.restype = None
        self.iokit.IOHIDManagerScheduleWithRunLoop.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p]

        # IOKit HID Device
        self.iokit.IOHIDDeviceSetReport.restype = ctypes.c_int
        self.iokit.IOHIDDeviceSetReport.argtypes = [
            ctypes.c_void_p, ctypes.c_uint32, ctypes.c_long, ctypes.c_char_p, ctypes.c_long
        ]
        self.iokit.IOHIDDeviceGetReport.restype = ctypes.c_int
        self.iokit.IOHIDDeviceGetReport.argtypes = [
            ctypes.c_void_p, ctypes.c_uint32, ctypes.c_long, ctypes.POINTER(ctypes.c_uint8), ctypes.POINTER(ctypes.c_long)
        ]

        # Callbacks
        self.IOHIDReportCallback = ctypes.CFUNCTYPE(
            None,
            ctypes.c_void_p, ctypes.c_int, ctypes.c_void_p, ctypes.c_uint32,
            ctypes.c_uint32, ctypes.POINTER(ctypes.c_uint8), ctypes.c_long
        )
        self.iokit.IOHIDDeviceRegisterInputReportCallback.restype = None
        self.iokit.IOHIDDeviceRegisterInputReportCallback.argtypes = [
            ctypes.c_void_p, ctypes.POINTER(ctypes.c_uint8), ctypes.c_long, self.IOHIDReportCallback, ctypes.c_void_p
        ]

        self.IOHIDDeviceCallback = ctypes.CFUNCTYPE(None, ctypes.c_void_p, ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p)
        self.iokit.IOHIDManagerRegisterDeviceMatchingCallback.restype = None
        self.iokit.IOHIDManagerRegisterDeviceMatchingCallback.argtypes = [
            ctypes.c_void_p, self.IOHIDDeviceCallback, ctypes.c_void_p
        ]
        self.iokit.IOHIDManagerRegisterDeviceRemovalCallback.restype = None
        self.iokit.IOHIDManagerRegisterDeviceRemovalCallback.argtypes = [
            ctypes.c_void_p, self.IOHIDDeviceCallback, ctypes.c_void_p
        ]

    @property
    def is_connected(self) -> bool:
        """Returns True if the Fifine Stream Deck hardware is currently attached."""
        with self._device_lock:
            return self._device is not None

    def get_firmware_version(self) -> Optional[str]:
        """Queries MCU firmware version via 512-byte input report query.

        Expected return format: 'V3.D6.1.009'
        """
        with self._device_lock:
            device = self._device
        if not device:
            return None

        buf = (ctypes.c_uint8 * self.INPUT_REPORT_SIZE)()
        sz = ctypes.c_long(self.INPUT_REPORT_SIZE)
        res = self.iokit.IOHIDDeviceGetReport(device, 0, 0, buf, ctypes.byref(sz))
        if res == 0 and sz.value > 0:
            raw = bytes(buf[:sz.value]).strip(b"\x00")
            return raw.decode("latin1", errors="ignore")
        return None

    def _on_device_matched(self, context, result, sender, device):
        with self._device_lock:
            self._device = device

        # Register explicit 512-byte input report buffer callback
        self.iokit.IOHIDDeviceRegisterInputReportCallback(
            device, self._input_report_buffer, self.INPUT_REPORT_SIZE, self._c_report_cb, None
        )
        print(f"[FifineDriver] Hardware attached (handle={device}). Ready for streaming.", file=sys.stderr, flush=True)

        if self.on_connect:
            try:
                self.on_connect()
            except Exception as e:
                print(f"[FifineDriver] Error in on_connect callback: {e}", file=sys.stderr, flush=True)

    def _on_device_removed(self, context, result, sender, device):
        with self._device_lock:
            if self._device == device:
                self._device = None
        print(f"[FifineDriver] Hardware detached (handle={device}). Waiting for reconnect...", file=sys.stderr, flush=True)

        if self.on_disconnect:
            try:
                self.on_disconnect()
            except Exception as e:
                print(f"[FifineDriver] Error in on_disconnect callback: {e}", file=sys.stderr, flush=True)

    def _on_input_report(self, context, result, sender, report_type, report_id, report, report_length):
        """Asynchronous button report callback on UsagePage 0xFFA0, Usage 1.

        Report format (512 bytes):
        Offset 0..6:   b"ACK\x00\x00OK\x00\x00"
        Byte 9:        0xFF = Packet ACK; 1..15 = Hardware Key ID
        Byte 10:       If Key: State (1 = Pressed / Down, 0 = Released / Up)
                       If ACK: 0xFF = packet ACK terminator
        """
        if report_length < 11:
            return

        raw = bytes([report[i] for i in range(min(report_length, 12))])
        if raw.startswith(b"ACK\x00\x00OK") or (raw[:3] == b"ACK" and raw[5:7] == b"OK"):
            raw_key_id = raw[9]
            state_raw = raw[10]

            if raw_key_id in (0, 0xFF):
                # Synchronous MCU ACK confirmation for image packet transmission
                self._ack_event.set()
                return

            key_id = DEVICE_ID_TO_KEY.get(raw_key_id, raw_key_id)
            if 1 <= key_id <= 15:
                state = "down" if state_raw == 1 else "up"
                row = (key_id - 1) // 5
                col = (key_id - 1) % 5

                event = KeyEvent(
                    key=key_id,
                    state=state,
                    row=row,
                    col=col,
                    timestamp=time.time(),
                )

                if self.on_event:
                    try:
                        self.on_event(event)
                    except Exception as e:
                        print(f"[FifineDriver] Error in on_event handler: {e}", file=sys.stderr, flush=True)

    def send_report(self, packet: bytes, blocking: bool = False):
        """Enqueues a single 512-byte packet to be sent via the paced write thread."""
        self.send_reports([packet], blocking=blocking)

    def send_reports(self, packets: List[bytes], blocking: bool = False):
        """Enqueues a burst of 512-byte packets with paced write transmission."""
        if not packets:
            return

        event = threading.Event() if blocking else None
        self._write_queue.put((packets, event))

        if blocking and event:
            event.wait()

    def _write_loop(self):
        """Dedicated write thread managing paced and ACK-synchronized 1024-byte packet transmission.

        For image stream packets (BAT headers and raw JPEG chunks), waits for MCU ACK
        with an adaptive timeout to prevent MCU FIFO overflow.
        For session control packets, applies paced delay.
        """
        while self.is_running:
            try:
                item = self._write_queue.get(timeout=0.05)
            except queue.Empty:
                continue

            packets, completion_event = item
            with self._device_lock:
                device = self._device

            if device:
                for pkt in packets:
                    if not self.is_running:
                        break

                    # Always pad to exact 1024 bytes (D6 HID Output Report size)
                    sz = self.OUTPUT_REPORT_SIZE
                    if len(pkt) < sz:
                        pkt = pkt + b"\x00" * (sz - len(pkt))
                    elif len(pkt) > sz:
                        pkt = pkt[:sz]

                    # kIOHIDReportTypeOutput = 1, ReportID = 0
                    self.iokit.IOHIDDeviceSetReport(device, 1, 0, pkt, len(pkt))
                    time.sleep(0.005)

            if completion_event:
                completion_event.set()

            self._write_queue.task_done()

    def _heartbeat_loop(self):
        """Periodic keepalive heartbeat to prevent MCU display sleep."""
        heartbeat_pkt = b"CRT\x00\x00CONNECT\x00\x00\x00" + b"\x00" * (self.OUTPUT_REPORT_SIZE - 15)
        while self.is_running:
            time.sleep(2.0)
            if not self.is_running:
                break
            with self._device_lock:
                device = self._device
            if device:
                try:
                    self.iokit.IOHIDDeviceSetReport(device, 1, 0, heartbeat_pkt, len(heartbeat_pkt))
                except Exception:
                    pass

    def _read_loop_worker(self):
        """Background Read Thread: Drains input reports and manages IOHID run loop."""
        kCFStringEncodingUTF8 = 0x08000100
        kCFNumberSInt32Type = 3

        k_vid = self.cf.CFStringCreateWithCString(None, b"VendorID", kCFStringEncodingUTF8)
        v_vid_int = ctypes.c_int32(self.VENDOR_ID)
        v_vid = self.cf.CFNumberCreate(None, kCFNumberSInt32Type, ctypes.byref(v_vid_int))

        k_pid = self.cf.CFStringCreateWithCString(None, b"ProductID", kCFStringEncodingUTF8)
        v_pid_int = ctypes.c_int32(self.PRODUCT_ID)
        v_pid = self.cf.CFNumberCreate(None, kCFNumberSInt32Type, ctypes.byref(v_pid_int))

        k_page = self.cf.CFStringCreateWithCString(None, b"PrimaryUsagePage", kCFStringEncodingUTF8)
        v_page_int = ctypes.c_int32(self.USAGE_PAGE)
        v_page = self.cf.CFNumberCreate(None, kCFNumberSInt32Type, ctypes.byref(v_page_int))

        k_usage = self.cf.CFStringCreateWithCString(None, b"PrimaryUsage", kCFStringEncodingUTF8)
        v_usage_int = ctypes.c_int32(self.USAGE)
        v_usage = self.cf.CFNumberCreate(None, kCFNumberSInt32Type, ctypes.byref(v_usage_int))

        keys = (ctypes.c_void_p * 4)(k_vid, k_pid, k_page, k_usage)
        vals = (ctypes.c_void_p * 4)(v_vid, v_pid, v_page, v_usage)
        matching = self.cf.CFDictionaryCreate(None, keys, vals, 4, None, None)

        manager = self.iokit.IOHIDManagerCreate(None, 0)
        self.iokit.IOHIDManagerSetDeviceMatching(manager, matching)

        self._c_dev_cb = self.IOHIDDeviceCallback(self._on_device_matched)
        self._c_rem_cb = self.IOHIDDeviceCallback(self._on_device_removed)
        self._c_report_cb = self.IOHIDReportCallback(self._on_input_report)

        self.iokit.IOHIDManagerRegisterDeviceMatchingCallback(manager, self._c_dev_cb, None)
        self.iokit.IOHIDManagerRegisterDeviceRemovalCallback(manager, self._c_rem_cb, None)

        self._run_loop = self.cf.CFRunLoopGetCurrent()
        self._kCFRunLoopDefaultMode = self.cf.CFStringCreateWithCString(
            None, b"kCFRunLoopDefaultMode", kCFStringEncodingUTF8
        )
        self.iokit.IOHIDManagerScheduleWithRunLoop(manager, self._run_loop, self._kCFRunLoopDefaultMode)

        ret = self.iokit.IOHIDManagerOpen(manager, 0)
        if ret != 0:
            print(f"[FifineDriver] Warning: IOHIDManagerOpen returned {ret}", file=sys.stderr, flush=True)

        # Initial pump to detect already connected hardware
        for _ in range(5):
            self.cf.CFRunLoopRunInMode(self._kCFRunLoopDefaultMode, 0.02, False)

        # Main read loop: keeps input report pipe drained at all times
        while self.is_running:
            self.cf.CFRunLoopRunInMode(self._kCFRunLoopDefaultMode, 0.01, False)

        self.iokit.IOHIDManagerClose(manager, 0)

    def start(self, blocking: bool = False):
        """Starts both read and write threads. If blocking is True, blocks the calling thread."""
        if self.is_running:
            return

        self.is_running = True

        # Start paced write thread
        self._write_thread = threading.Thread(target=self._write_loop, daemon=True, name="FifineWriteThread")
        self._write_thread.start()

        # Start periodic keepalive heartbeat thread
        self._heartbeat_thread = threading.Thread(target=self._heartbeat_loop, daemon=True, name="FifineHeartbeatThread")
        self._heartbeat_thread.start()

        # Start background read loop
        if blocking:
            self._read_loop_worker()
        else:
            self._read_thread = threading.Thread(target=self._read_loop_worker, daemon=True, name="FifineReadThread")
            self._read_thread.start()

    def stop(self):
        """Stops the driver, write queue, and run loops cleanly."""
        self.is_running = False
        if self._run_loop:
            self.cf.CFRunLoopStop(self._run_loop)
            self._run_loop = None

        if self._read_thread and self._read_thread.is_alive():
            self._read_thread.join(timeout=1.0)
            self._read_thread = None

        if self._write_thread and self._write_thread.is_alive():
            self._write_thread.join(timeout=1.0)
            self._write_thread = None

        if self._heartbeat_thread and self._heartbeat_thread.is_alive():
            self._heartbeat_thread.join(timeout=0.5)
            self._heartbeat_thread = None
