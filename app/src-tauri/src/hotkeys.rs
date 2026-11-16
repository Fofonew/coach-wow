//! Global keyboard and mouse hooks (WH_KEYBOARD_LL / WH_MOUSE_LL). Events are passed through
//! to the game untouched; we only observe them.

use crossbeam_channel::Sender;
use parking_lot::Mutex;
use std::collections::HashSet;
use std::sync::OnceLock;

#[derive(Clone, Debug)]
pub struct InputEvent {
    /// Binding names for this input: the main one first, then legacy aliases.
    pub names: Vec<String>,
    pub pressed: bool,
}

struct Capture {
    sender: Sender<String>,
    pending_modifier: Option<String>,
}

static SINK: OnceLock<Sender<InputEvent>> = OnceLock::new();
static CAPTURE: Mutex<Option<Capture>> = Mutex::new(None);
static HELD: Mutex<Option<HashSet<String>>> = Mutex::new(None);

const MODIFIERS: [u32; 8] = [0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5, 0x5B, 0x5C];

fn named_key(vk: u32) -> Option<String> {
    Some(match vk {
        0x08 => "backspace".into(),
        0x09 => "tab".into(),
        0x0D => "enter".into(),
        0x13 => "pause".into(),
        0x14 => "caps_lock".into(),
        0x1B => "esc".into(),
        0x20 => "space".into(),
        0x21 => "page_up".into(),
        0x22 => "page_down".into(),
        0x23 => "end".into(),
        0x24 => "home".into(),
        0x25 => "left".into(),
        0x26 => "up".into(),
        0x27 => "right".into(),
        0x28 => "down".into(),
        0x2C => "print_screen".into(),
        0x2D => "insert".into(),
        0x2E => "delete".into(),
        0x30..=0x39 => ((b'0' + (vk - 0x30) as u8) as char).to_string(),
        0x41..=0x5A => ((b'a' + (vk - 0x41) as u8) as char).to_string(),
        0x5B => "cmd_l".into(),
        0x5C => "cmd_r".into(),
        0x5D => "menu".into(),
        0x60..=0x69 => format!("num{}", vk - 0x60),
        0x6A => "num_multiply".into(),
        0x6B => "num_add".into(),
        0x6D => "num_subtract".into(),
        0x6E => "num_decimal".into(),
        0x6F => "num_divide".into(),
        0x70..=0x87 => format!("f{}", vk - 0x6F),
        0x90 => "num_lock".into(),
        0x91 => "scroll_lock".into(),
        0xA0 => "shift_l".into(),
        0xA1 => "shift_r".into(),
        0xA2 => "ctrl_l".into(),
        0xA3 => "ctrl_r".into(),
        0xA4 => "alt_l".into(),
        0xA5 => "alt_r".into(),
        0xAD => "media_volume_mute".into(),
        0xB3 => "media_play_pause".into(),
        _ => return None,
    })
}

/// Character printed by a punctuation key on the current keyboard layout (`, ², ;, ù...).
fn layout_char(vk: u32) -> Option<String> {
    #[cfg(windows)]
    {
        use windows::Win32::UI::Input::KeyboardAndMouse::{MapVirtualKeyW, MAPVK_VK_TO_CHAR};
        let code = unsafe { MapVirtualKeyW(vk, MAPVK_VK_TO_CHAR) } & 0x7FFF_FFFF;
        let character = char::from_u32(code)?;
        if character.is_whitespace() || character.is_control() || code == 0 {
            return None;
        }
        return Some(character.to_lowercase().to_string());
    }
    #[allow(unreachable_code)]
    None
}

pub fn key_names(vk: u32) -> Vec<String> {
    let legacy = format!("key:vk{vk}");
    match named_key(vk).or_else(|| layout_char(vk)) {
        Some(name) => vec![format!("key:{name}"), legacy],
        None => vec![legacy],
    }
}

fn emit(names: Vec<String>, pressed: bool, modifier: bool) {
    let primary = names[0].clone();
    {
        let mut capture = CAPTURE.lock();
        if let Some(state) = capture.as_mut() {
            if pressed && modifier {
                state.pending_modifier.get_or_insert(primary);
                return;
            }
            if pressed {
                let _ = capture.take().map(|state| state.sender.send(primary));
                return;
            }
            if modifier && state.pending_modifier.as_deref() == Some(primary.as_str()) {
                let _ = capture.take().map(|state| state.sender.send(primary));
                return;
            }
        }
    }
    {
        let mut held = HELD.lock();
        let held = held.get_or_insert_with(HashSet::new);
        if pressed {
            if !held.insert(primary) {
                return;
            }
        } else if !held.remove(&primary) {
            return;
        }
    }
    if let Some(sink) = SINK.get() {
        let _ = sink.send(InputEvent { names, pressed });
    }
}

/// Waits for the next key or mouse button press and returns its binding name.
/// A modifier (Ctrl, Alt, Shift, Win) is only captured when released alone, so
/// Alt Gr combinations give the real key.
pub fn capture_next(timeout: std::time::Duration) -> Option<String> {
    let (sender, receiver) = crossbeam_channel::bounded(1);
    *CAPTURE.lock() = Some(Capture { sender, pending_modifier: None });
    let result = receiver.recv_timeout(timeout).ok();
    CAPTURE.lock().take();
    result
}

#[cfg(windows)]
mod platform {
    use super::{emit, key_names, MODIFIERS};
    use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetMessageW, SetWindowsHookExW, HC_ACTION, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT, WH_KEYBOARD_LL,
        WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN,
        WM_XBUTTONUP,
    };

    unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code == HC_ACTION as i32 {
            let message = wparam.0 as u32;
            let data = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
            let modifier = MODIFIERS.contains(&data.vkCode);
            match message {
                WM_KEYDOWN | WM_SYSKEYDOWN => emit(key_names(data.vkCode), true, modifier),
                WM_KEYUP | WM_SYSKEYUP => emit(key_names(data.vkCode), false, modifier),
                _ => {}
            }
        }
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }

    unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code == HC_ACTION as i32 {
            let message = wparam.0 as u32;
            match message {
                WM_XBUTTONDOWN | WM_XBUTTONUP => {
                    let data = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
                    let button = (data.mouseData >> 16) & 0xFFFF;
                    let name = if button == 1 { "mouse:x1" } else { "mouse:x2" };
                    emit(vec![name.to_string()], message == WM_XBUTTONDOWN, false);
                }
                WM_MBUTTONDOWN => emit(vec!["mouse:middle".into()], true, false),
                WM_MBUTTONUP => emit(vec!["mouse:middle".into()], false, false),
                _ => {}
            }
        }
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }

    pub fn run() {
        unsafe {
            let module = GetModuleHandleW(None).ok();
            let instance = module.map(|module| HINSTANCE(module.0));
            if let Err(error) = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), instance, 0) {
                log::error!("Keyboard hook failed: {error}");
            }
            if let Err(error) = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), instance, 0) {
                log::error!("Mouse hook failed: {error}");
            }
            let mut message = MSG::default();
            while GetMessageW(&mut message, None, 0, 0).as_bool() {}
        }
    }
}

pub fn start(sink: Sender<InputEvent>) {
    if SINK.set(sink).is_err() {
        return;
    }
    std::thread::Builder::new()
        .name("input-hooks".into())
        .spawn(|| {
            #[cfg(windows)]
            platform::run();
        })
        .expect("spawn input hook thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_punctuation_keys_by_character() {
        let names = key_names(0xC0);
        assert_eq!(names.last().unwrap(), "key:vk192");
        assert!(!names[0].contains("vk"), "{names:?}");
        assert_eq!(key_names(0x7C)[0], "key:f13");
    }
}
