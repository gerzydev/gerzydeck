"""High-level pure Python controller for Fifine AmpliGame Stream Deck (D6 / D6-Pro).

Provides an ergonomic Python API to control the 15-key LCD display,
listen to button events, manage brightness, and stream key graphics.
Works 100% standalone over USB with zero background daemons or binary dependencies.
"""

import signal
import sys
import threading
import time
from typing import Callable, Dict, List, Optional, Union, Any

from .driver import FifineDriver
from .display import FifineDisplay, DEFAULT_KEY_WIDTH, DEFAULT_KEY_HEIGHT, NUM_KEYS
from .events import KeyEvent


class FifineDeck:
    """High-level controller interface for the Fifine AmpliGame Stream Deck.

    Example:
        deck = FifineDeck()

        # Set custom key icon or color
        deck.set_key_image(1, "icons/play.png")
        deck.set_key_color(2, "#3b82f6")
        deck.set_brightness(100)

        @deck.on_press
        def handle_press(key: int):
            print(f"Key {key} pressed!")

        @deck.on_release
        def handle_release(key: int):
            print(f"Key {key} released!")

        deck.start(blocking=True)
    """

    def __init__(self, auto_wake: bool = True, default_brightness: int = 100):
        self.auto_wake = auto_wake
        self.default_brightness = max(0, min(100, int(default_brightness)))

        self._press_callbacks: List[Callable[[int], None]] = []
        self._release_callbacks: List[Callable[[int], None]] = []
        self._event_callbacks: List[Callable[[KeyEvent], None]] = []

        self._driver = FifineDriver(
            on_event=self._dispatch_event,
            on_connect=self._on_device_connected,
            on_disconnect=self._on_device_disconnected,
        )
        self._display = FifineDisplay(self._driver, DEFAULT_KEY_WIDTH, DEFAULT_KEY_HEIGHT)

        self._is_running = False
        self._wake_thread: Optional[threading.Thread] = None

    @property
    def is_connected(self) -> bool:
        """Returns True if the physical hardware is currently connected."""
        return self._driver.is_connected

    @property
    def display(self) -> FifineDisplay:
        """Access the underlying LCD framebuffer engine."""
        return self._display

    def get_firmware_version(self) -> Optional[str]:
        """Queries the MCU firmware version (e.g. 'V3.D6.1.009')."""
        return self._driver.get_firmware_version()

    def on_press(self, callback: Callable[[int], None]) -> Callable[[int], None]:
        """Decorator or method to register a button press callback."""
        self._press_callbacks.append(callback)
        return callback

    def on_release(self, callback: Callable[[int], None]) -> Callable[[int], None]:
        """Decorator or method to register a button release callback."""
        self._release_callbacks.append(callback)
        return callback

    def on_event(self, callback: Callable[[KeyEvent], None]) -> Callable[[KeyEvent], None]:
        """Decorator or method to register a raw KeyEvent callback."""
        self._event_callbacks.append(callback)
        return callback

    def _dispatch_event(self, event: KeyEvent):
        """Internal dispatcher routing raw button reports to callbacks."""
        for cb in self._event_callbacks:
            try:
                cb(event)
            except Exception as e:
                print(f"[FifineDeck Error in on_event]: {e}", file=sys.stderr)

        if event.is_press:
            for cb in self._press_callbacks:
                try:
                    cb(event.key)
                except Exception as e:
                    print(f"[FifineDeck Error in on_press]: {e}", file=sys.stderr)
        elif event.is_release:
            for cb in self._release_callbacks:
                try:
                    cb(event.key)
                except Exception as e:
                    print(f"[FifineDeck Error in on_release]: {e}", file=sys.stderr)

    def _on_device_connected(self):
        """Triggered when USB hardware is attached."""
        if self.auto_wake:
            # Spawn in dedicated worker thread to keep input read pipe draining concurrently
            self._wake_thread = threading.Thread(
                target=self._wake_worker,
                daemon=True,
                name="FifineWakeThread"
            )
            self._wake_thread.start()

    def _wake_worker(self):
        """Wakes the hardware display asynchronously while read pipe drains continuously."""
        try:
            self._display.wake(brightness=self.default_brightness)
        except Exception as e:
            print(f"[FifineDeck] Error during auto-wake: {e}", file=sys.stderr)

    def _on_device_disconnected(self):
        """Triggered when USB hardware is detached."""
        pass

    def wake(self, initial_keys: Optional[Dict[int, Any]] = None, brightness: Optional[int] = None):
        """Manually triggers the master cold-boot display wake sequence."""
        br = self.default_brightness if brightness is None else brightness
        self._display.wake(initial_keys=initial_keys, brightness=br)

    def set_brightness(self, level: int):
        """Sets display brightness (0..100%)."""
        self._display.set_brightness(level)

    def set_key_image(self, key_id: int, image: Any, blocking: bool = False):
        """Uploads an image (file path, JPEG bytes, PIL Image, or RGB color) to a key (1..15)."""
        self._display.set_key_image(key_id, image, immediate=True, blocking=blocking)

    def set_key_color(self, key_id: int, color: Union[str, Tuple[int, int, int]], blocking: bool = False):
        """Sets a specific key tile to a solid color."""
        self._display.set_key_color(key_id, color, immediate=True, blocking=blocking)

    def fill_color(self, color: Union[str, Tuple[int, int, int]]):
        """Fills all 15 key tiles with a solid color."""
        self._display.fill_color(color, immediate=True)

    def clear(self, color: Union[str, Tuple[int, int, int]] = (0, 0, 0)):
        """Clears all 15 key tiles to black or specified color."""
        self._display.clear(color, immediate=True)

    def refresh_display(self):
        """Re-streams the entire active framebuffer to the physical deck."""
        self._display.refresh_display()

    def start(self, blocking: bool = False):
        """Starts the native hardware driver listener and display pipeline."""
        if self._is_running:
            return

        self._is_running = True

        if blocking:
            def handle_sig(sig, frame):
                self.stop()
                sys.exit(0)

            signal.signal(signal.SIGINT, handle_sig)
            signal.signal(signal.SIGTERM, handle_sig)

            self._driver.start(blocking=True)
        else:
            self._driver.start(blocking=False)

    def stop(self):
        """Stops the hardware driver and display engine."""
        self._is_running = False
        if self._driver:
            self._driver.stop()

    def __enter__(self):
        self.start(blocking=False)
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.stop()
