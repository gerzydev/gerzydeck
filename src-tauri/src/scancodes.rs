//! KeyboardEvent.code → PC/AT set-1 scan code (+ extended flag).

const PLAIN: &[(&str, u16)] = &[
    ("Escape", 0x01), ("Digit1", 0x02), ("Digit2", 0x03), ("Digit3", 0x04), ("Digit4", 0x05), ("Digit5", 0x06),
    ("Digit6", 0x07), ("Digit7", 0x08), ("Digit8", 0x09), ("Digit9", 0x0A), ("Digit0", 0x0B), ("Minus", 0x0C),
    ("Equal", 0x0D), ("Backspace", 0x0E), ("Tab", 0x0F), ("KeyQ", 0x10), ("KeyW", 0x11), ("KeyE", 0x12),
    ("KeyR", 0x13), ("KeyT", 0x14), ("KeyY", 0x15), ("KeyU", 0x16), ("KeyI", 0x17), ("KeyO", 0x18), ("KeyP", 0x19),
    ("BracketLeft", 0x1A), ("BracketRight", 0x1B), ("Enter", 0x1C), ("ControlLeft", 0x1D), ("KeyA", 0x1E),
    ("KeyS", 0x1F), ("KeyD", 0x20), ("KeyF", 0x21), ("KeyG", 0x22), ("KeyH", 0x23), ("KeyJ", 0x24), ("KeyK", 0x25),
    ("KeyL", 0x26), ("Semicolon", 0x27), ("Quote", 0x28), ("Backquote", 0x29), ("ShiftLeft", 0x2A),
    ("Backslash", 0x2B), ("KeyZ", 0x2C), ("KeyX", 0x2D), ("KeyC", 0x2E), ("KeyV", 0x2F), ("KeyB", 0x30),
    ("KeyN", 0x31), ("KeyM", 0x32), ("Comma", 0x33), ("Period", 0x34), ("Slash", 0x35), ("ShiftRight", 0x36),
    ("NumpadMultiply", 0x37), ("AltLeft", 0x38), ("Space", 0x39), ("CapsLock", 0x3A), ("F1", 0x3B), ("F2", 0x3C),
    ("F3", 0x3D), ("F4", 0x3E), ("F5", 0x3F), ("F6", 0x40), ("F7", 0x41), ("F8", 0x42), ("F9", 0x43), ("F10", 0x44),
    ("NumLock", 0x45), ("ScrollLock", 0x46), ("Numpad7", 0x47), ("Numpad8", 0x48), ("Numpad9", 0x49),
    ("NumpadSubtract", 0x4A), ("Numpad4", 0x4B), ("Numpad5", 0x4C), ("Numpad6", 0x4D), ("NumpadAdd", 0x4E),
    ("Numpad1", 0x4F), ("Numpad2", 0x50), ("Numpad3", 0x51), ("Numpad0", 0x52), ("NumpadDecimal", 0x53),
    ("IntlBackslash", 0x56), ("F11", 0x57), ("F12", 0x58), ("F13", 0x64), ("F14", 0x65), ("F15", 0x66),
    ("F16", 0x67), ("F17", 0x68), ("F18", 0x69), ("F19", 0x6A), ("F20", 0x6B), ("F21", 0x6C), ("F22", 0x6D),
    ("F23", 0x6E), ("F24", 0x76),
];

const EXTENDED: &[(&str, u16)] = &[
    ("NumpadEnter", 0x1C), ("ControlRight", 0x1D), ("NumpadDivide", 0x35), ("PrintScreen", 0x37),
    ("AltRight", 0x38), ("Home", 0x47), ("ArrowUp", 0x48), ("PageUp", 0x49), ("ArrowLeft", 0x4B),
    ("ArrowRight", 0x4D), ("End", 0x4F), ("ArrowDown", 0x50), ("PageDown", 0x51), ("Insert", 0x52),
    ("Delete", 0x53), ("MetaLeft", 0x5B), ("MetaRight", 0x5C), ("ContextMenu", 0x5D),
];

pub const MODIFIERS: &[&str] = &[
    "ControlLeft", "ControlRight", "ShiftLeft", "ShiftRight", "AltLeft", "AltRight", "MetaLeft", "MetaRight",
];

/// → (scan code, extended)
pub fn scancode(code: &str) -> Option<(u16, bool)> {
    PLAIN.iter().find(|(c, _)| *c == code).map(|&(_, s)| (s, false))
        .or_else(|| EXTENDED.iter().find(|(c, _)| *c == code).map(|&(_, s)| (s, true)))
}

pub fn all_codes() -> Vec<&'static str> {
    PLAIN.iter().chain(EXTENDED).map(|(c, _)| *c).collect()
}

pub fn media_vk(command: &str) -> Option<u16> {
    Some(match command {
        "playpause" => 0xB3,
        "next" => 0xB0,
        "prev" => 0xB1,
        "mute" => 0xAD,
        "volup" => 0xAF,
        "voldown" => 0xAE,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spot_checks() {
        assert_eq!(scancode("KeyA"), Some((0x1E, false)));
        assert_eq!(scancode("KeyZ"), Some((0x2C, false)));
        assert_eq!(scancode("KeyY"), Some((0x15, false)));
        assert_eq!(scancode("Digit1"), Some((0x02, false)));
        assert_eq!(scancode("Digit0"), Some((0x0B, false)));
        assert_eq!(scancode("F1"), Some((0x3B, false)));
        assert_eq!(scancode("F11"), Some((0x57, false)));
        assert_eq!(scancode("F13"), Some((0x64, false)));
        assert_eq!(scancode("F24"), Some((0x76, false)));
        assert_eq!(scancode("ArrowUp"), Some((0x48, true)));
        assert_eq!(scancode("ControlRight"), Some((0x1D, true)));
        assert_eq!(scancode("NumpadEnter"), Some((0x1C, true)));
        assert_eq!(scancode("MetaLeft"), Some((0x5B, true)));
        assert_eq!(scancode("Escape"), Some((0x01, false)));
        assert_eq!(scancode("Bogus"), None);
        assert!(all_codes().len() > 100);
    }

    #[test]
    fn media() {
        assert_eq!(media_vk("playpause"), Some(0xB3));
        assert_eq!(media_vk("next"), Some(0xB0));
        assert_eq!(media_vk("prev"), Some(0xB1));
        assert_eq!(media_vk("mute"), Some(0xAD));
        assert_eq!(media_vk("volup"), Some(0xAF));
        assert_eq!(media_vk("voldown"), Some(0xAE));
        assert_eq!(media_vk("x"), None);
    }
}
