//! Language of the texts the core shows (errors, key texts, tray): English by default, Czech optional.
//! The engine sets it from the config, so every thread sees the user's choice.

use std::sync::atomic::{AtomicBool, Ordering};

static CZECH: AtomicBool = AtomicBool::new(false);

pub fn set_language(lang: &str) {
    CZECH.store(lang == "cs", Ordering::Relaxed);
}

pub fn is_czech() -> bool {
    CZECH.load(Ordering::Relaxed)
}

/// `tr("česky", "English")` → the text in the current language.
pub fn tr(cs: &'static str, en: &'static str) -> &'static str {
    pick(is_czech(), cs, en)
}

fn pick(czech: bool, cs: &'static str, en: &'static str) -> &'static str {
    if czech { cs } else { en }
}

/// `trf!("česky {x}", "English {x}")` → formatted text in the current language.
#[macro_export]
macro_rules! trf {
    ($cs:literal, $en:literal $(, $arg:expr)* $(,)?) => {
        if $crate::i18n::is_czech() { format!($cs $(, $arg)*) } else { format!($en $(, $arg)*) }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_language() {
        // the global switch is not touched here: other tests run in parallel
        assert_eq!(pick(false, "ahoj", "hello"), "hello");
        assert_eq!(pick(true, "ahoj", "hello"), "ahoj");
        assert!(!is_czech(), "English is the default");
    }

    #[test]
    fn config_language_is_en_or_cs() {
        use crate::config::normalize_config;
        let base = serde_json::json!({ "profiles": [{ "id": "A", "name": "A", "keys": {} }] });
        assert_eq!(normalize_config(&base).language, "en");
        let mut cs = base.clone();
        cs["language"] = "cs".into();
        assert_eq!(normalize_config(&cs).language, "cs");
        let mut odd = base;
        odd["language"] = "klingon".into();
        assert_eq!(normalize_config(&odd).language, "en");
        assert_eq!(crate::config::create_default_config().language, "en");
    }
}
