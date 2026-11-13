//! Executes key actions. Backends (keyboard, launch, system, profile switch) are injected.

use crate::config::{Action, Step};
pub use crate::keyboard::Keyboard;
use crate::scancodes::media_vk;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

pub type LaunchFn = Arc<dyn Fn(&str, &[String]) + Send + Sync>;
pub type CommandFn = Arc<dyn Fn(&str) + Send + Sync>;

pub struct ActionRunner {
    keyboard: Arc<dyn Keyboard>,
    launch: LaunchFn,
    system: CommandFn,
    switch_profile: CommandFn,
    /// deck key → codes pressed by a `keys` action (released exactly, even after a profile switch)
    held: Mutex<HashMap<u8, Vec<String>>>,
    macros: Arc<Mutex<HashSet<u8>>>,
}

impl ActionRunner {
    pub fn new(keyboard: Arc<dyn Keyboard>, launch: LaunchFn, system: CommandFn, switch_profile: CommandFn) -> Self {
        Self { keyboard, launch, system, switch_profile, held: Mutex::default(), macros: Arc::default() }
    }

    pub fn is_macro_running(&self, key: u8) -> bool {
        self.macros.lock().unwrap().contains(&key)
    }

    /// Returns the macro thread handle when a macro was started.
    pub fn press(&self, key: u8, action: &Action) -> Option<JoinHandle<()>> {
        match action {
            // plugin and sound actions are routed by the engine (plugin host, sound thread), not executed here
            Action::None | Action::Plugin { .. } | Action::Usage { .. } | Action::Sound { .. } => {}
            Action::Keys { codes } => {
                let mut held = self.held.lock().unwrap();
                if held.contains_key(&key) {
                    return None;
                }
                for c in codes {
                    self.keyboard.down(c);
                }
                held.insert(key, codes.clone());
            }
            Action::Macro { steps } => {
                if !self.macros.lock().unwrap().insert(key) {
                    return None;
                }
                let kb = self.keyboard.clone();
                let macros = self.macros.clone();
                let steps = steps.clone();
                return std::thread::Builder::new()
                    .name(format!("macro-{key}"))
                    .spawn(move || {
                        run_macro(kb.as_ref(), &steps);
                        macros.lock().unwrap().remove(&key);
                    })
                    .map_err(|e| log::error!("macro thread failed: {e}"))
                    .ok();
            }
            Action::Launch { target, args } => (self.launch)(target, args),
            Action::Media { command } => {
                if let Some(vk) = media_vk(command) {
                    self.keyboard.tap_vk(vk);
                }
            }
            Action::System { command } => (self.system)(command),
            Action::Profile { profile_id } => (self.switch_profile)(profile_id),
        }
        None
    }

    pub fn release(&self, key: u8) {
        let codes = self.held.lock().unwrap().remove(&key);
        for c in codes.into_iter().flatten().rev() {
            self.keyboard.up(&c);
        }
    }

    pub fn release_all(&self) {
        let all: Vec<u8> = self.held.lock().unwrap().keys().copied().collect();
        for k in all {
            self.release(k);
        }
    }
}

fn run_macro(kb: &dyn Keyboard, steps: &[Step]) {
    let mut down: Vec<&str> = Vec::new();
    for s in steps {
        match s {
            Step::Tap { code } => {
                kb.down(code);
                kb.up(code);
            }
            Step::Down { code } => {
                kb.down(code);
                down.push(code);
            }
            Step::Up { code } => {
                kb.up(code);
                down.retain(|c| c != code);
            }
            Step::Wait { ms } => std::thread::sleep(Duration::from_millis(*ms as u64)),
            Step::Text { text } => kb.type_text(text),
        }
    }
    for c in down.into_iter().rev() {
        kb.up(c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Action, Step};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    #[derive(Default)]
    struct Rec(Mutex<Vec<String>>);
    impl Rec {
        fn push(&self, s: String) {
            self.0.lock().unwrap().push(s);
        }
        fn get(&self) -> Vec<String> {
            self.0.lock().unwrap().clone()
        }
    }
    struct FakeKb(Arc<Rec>);
    impl Keyboard for FakeKb {
        fn down(&self, c: &str) { self.0.push(format!("down {c}")) }
        fn up(&self, c: &str) { self.0.push(format!("up {c}")) }
        fn tap_vk(&self, vk: u16) { self.0.push(format!("vk {vk:#x}")) }
        fn type_text(&self, t: &str) { self.0.push(format!("text {t}")) }
    }

    fn setup() -> (ActionRunner, Arc<Rec>, Arc<Rec>) {
        let kb = Arc::new(Rec::default());
        let other = Arc::new(Rec::default());
        let (o1, o2, o3) = (other.clone(), other.clone(), other.clone());
        let r = ActionRunner::new(
            Arc::new(FakeKb(kb.clone())),
            Arc::new(move |t: &str, a: &[String]| o1.push(format!("launch {t} {}", a.join(",")))),
            Arc::new(move |c: &str| o2.push(format!("system {c}"))),
            Arc::new(move |id: &str| o3.push(format!("switch {id}"))),
        );
        (r, kb, other)
    }

    fn keys(codes: &[&str]) -> Action {
        Action::Keys { codes: codes.iter().map(|s| s.to_string()).collect() }
    }

    #[test]
    fn keys_down_in_order_up_in_reverse() {
        let (r, kb, _) = setup();
        r.press(1, &keys(&["ShiftLeft", "Digit3"]));
        assert_eq!(kb.get(), vec!["down ShiftLeft", "down Digit3"]);
        r.release(1);
        assert_eq!(kb.get()[2..], ["up Digit3", "up ShiftLeft"]);
    }

    #[test]
    fn release_lifts_codes_from_press_time_after_profile_switch() {
        let (r, kb, other) = setup();
        r.press(1, &keys(&["ShiftLeft"]));
        r.press(2, &Action::Profile { profile_id: "p2".into() });
        r.release(1);
        assert_eq!(kb.get(), vec!["down ShiftLeft", "up ShiftLeft"]);
        assert_eq!(other.get(), vec!["switch p2"]);
    }

    #[test]
    fn macro_runs_in_order_waits_auto_releases_and_ignores_repress() {
        let (r, kb, _) = setup();
        let m = Action::Macro {
            steps: vec![
                Step::Tap { code: "KeyQ".into() },
                Step::Wait { ms: 30 },
                Step::Down { code: "KeyE".into() },
            ],
        };
        let t0 = Instant::now();
        let h = r.press(3, &m).expect("macro thread");
        assert!(r.is_macro_running(3));
        assert!(r.press(3, &m).is_none());
        h.join().unwrap();
        assert!(t0.elapsed() >= Duration::from_millis(28));
        assert_eq!(kb.get(), vec!["down KeyQ", "up KeyQ", "down KeyE", "up KeyE"]);
        assert!(!r.is_macro_running(3));
    }

    #[test]
    fn macro_text_step() {
        let (r, kb, _) = setup();
        r.press(1, &Action::Macro { steps: vec![Step::Text { text: "ěšč".into() }] }).unwrap().join().unwrap();
        assert_eq!(kb.get(), vec!["text ěšč"]);
    }

    #[test]
    fn release_all_lifts_everything_once() {
        let (r, kb, _) = setup();
        r.press(1, &keys(&["KeyW"]));
        r.press(2, &keys(&["KeyA"]));
        r.release_all();
        let mut ups = kb.get()[2..].to_vec();
        ups.sort();
        assert_eq!(ups, vec!["up KeyA", "up KeyW"]);
        r.release(1);
        assert_eq!(kb.get().len(), 4);
    }

    #[test]
    fn one_shot_actions_fire_on_press_only() {
        let (r, kb, other) = setup();
        r.press(1, &Action::Profile { profile_id: "p2".into() });
        r.press(2, &Action::Launch { target: "notepad.exe".into(), args: vec!["a.txt".into()] });
        r.press(3, &Action::Media { command: "volup".into() });
        r.press(4, &Action::System { command: "lock".into() });
        for k in 1..=4 {
            r.release(k);
        }
        assert_eq!(other.get(), vec!["switch p2", "launch notepad.exe a.txt", "system lock"]);
        assert_eq!(kb.get(), vec!["vk 0xaf"]);
    }

    #[test]
    fn double_press_of_keys_action_does_not_double_down() {
        let (r, kb, _) = setup();
        r.press(1, &keys(&["KeyQ"]));
        r.press(1, &keys(&["KeyQ"]));
        r.release(1);
        assert_eq!(kb.get(), vec!["down KeyQ", "up KeyQ"]);
    }
}
