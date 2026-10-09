//! Injection clavier et souris par `SendInput` (§7.8).
//!
//! Limite UIPI (EF-43) : Windows ignore les entrées injectées vers une fenêtre
//! exécutée en administrateur si Mudra ne l'est pas. `SendInput` ne signale pas
//! cet échec : l'action est simplement sans effet.

use mudra_config::MouseTarget;
use mudra_config::keys::{Key, KeyCombo};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MAPVK_VK_TO_VSC, MOUSE_EVENT_FLAGS,
    MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE,
    MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL,
    MOUSEINPUT, MapVirtualKeyW, SendInput, VIRTUAL_KEY,
};

use crate::ActionError;

fn send(inputs: &[INPUT]) -> Result<(), ActionError> {
    // SAFETY: tableau d'INPUT correctement initialisés, taille de structure exacte.
    let sent = unsafe { SendInput(inputs, size_of::<INPUT>() as i32) };
    if sent as usize == inputs.len() {
        Ok(())
    } else {
        Err(ActionError::Input(format!(
            "SendInput a injecté {sent} événements sur {} (session verrouillée ou entrée bloquée ?)",
            inputs.len()
        )))
    }
}

fn key_input(key: Key, up: bool) -> INPUT {
    // SAFETY: simple consultation de la disposition clavier.
    let scan = unsafe { MapVirtualKeyW(u32::from(key.vk), MAPVK_VK_TO_VSC) } as u16;
    let mut flags = KEYBD_EVENT_FLAGS(0);
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    if key.extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    // Scancodes quand ils existent (§7.8) ; les touches multimédia n'en ont pas
    // toujours et passent alors par leur code virtuel.
    let (vk, scan) = if scan != 0 {
        flags |= KEYEVENTF_SCANCODE;
        (VIRTUAL_KEY(0), scan)
    } else {
        (VIRTUAL_KEY(key.vk), 0)
    };
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Appuie sur les touches dans l'ordre, puis les relâche en ordre inverse.
pub fn send_combo(combo: &KeyCombo) -> Result<(), ActionError> {
    let mut inputs = Vec::with_capacity(combo.keys.len() * 2);
    inputs.extend(combo.keys.iter().map(|k| key_input(*k, false)));
    inputs.extend(combo.keys.iter().rev().map(|k| key_input(*k, true)));
    send(&inputs)
}

/// Envoie une touche seule par son code virtuel (touches multimédia et volume).
pub fn send_vk(vk: u16) -> Result<(), ActionError> {
    send_combo(&KeyCombo {
        keys: vec![Key {
            vk,
            extended: true,
            modifier: false,
        }],
    })
}

fn mouse_input(flags: MOUSE_EVENT_FLAGS, dx: i32, dy: i32, data: i32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data as u32,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Déplacement absolu : `x`, `y` dans [0, 1] sur l'écran principal ou le bureau virtuel.
pub fn mouse_move(x: f32, y: f32, target: MouseTarget) -> Result<(), ActionError> {
    let to_abs = |v: f32| (v.clamp(0.0, 1.0) * 65535.0).round() as i32;
    let mut flags = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE;
    if target == MouseTarget::VirtualDesktop {
        flags |= MOUSEEVENTF_VIRTUALDESK;
    }
    send(&[mouse_input(flags, to_abs(x), to_abs(y), 0)])
}

pub fn mouse_left(down: bool) -> Result<(), ActionError> {
    send(&[mouse_input(
        if down {
            MOUSEEVENTF_LEFTDOWN
        } else {
            MOUSEEVENTF_LEFTUP
        },
        0,
        0,
        0,
    )])
}

pub fn mouse_left_click() -> Result<(), ActionError> {
    send(&[
        mouse_input(MOUSEEVENTF_LEFTDOWN, 0, 0, 0),
        mouse_input(MOUSEEVENTF_LEFTUP, 0, 0, 0),
    ])
}

pub fn mouse_right_click() -> Result<(), ActionError> {
    send(&[
        mouse_input(MOUSEEVENTF_RIGHTDOWN, 0, 0, 0),
        mouse_input(MOUSEEVENTF_RIGHTUP, 0, 0, 0),
    ])
}

/// Défilement vertical : `notches` crans, positif vers le haut.
pub fn mouse_scroll(notches: i32) -> Result<(), ActionError> {
    send(&[mouse_input(MOUSEEVENTF_WHEEL, 0, 0, notches * 120)])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test matériel : injecte F24, une touche sans effet sur un clavier courant.
    #[test]
    #[ignore = "injecte une touche dans la session"]
    fn injects_a_harmless_key() {
        send_combo(&KeyCombo::parse("F24").unwrap()).expect("SendInput");
    }

    #[test]
    fn key_events_use_scancodes_and_extended_flag() {
        let win = key_input(
            Key {
                vk: 0x5B,
                extended: true,
                modifier: true,
            },
            false,
        );
        // SAFETY: l'INPUT vient d'être construit comme un événement clavier.
        let ki = unsafe { win.Anonymous.ki };
        assert!(ki.dwFlags.contains(KEYEVENTF_EXTENDEDKEY));
        assert!(!ki.dwFlags.contains(KEYEVENTF_KEYUP));
        let a_up = key_input(
            Key {
                vk: b'A' as u16,
                extended: false,
                modifier: false,
            },
            true,
        );
        // SAFETY: idem.
        let ki = unsafe { a_up.Anonymous.ki };
        assert!(ki.dwFlags.contains(KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP));
        // Le scancode de « A » dépend de la disposition (0x1E en QWERTY, 0x10 en AZERTY).
        assert_ne!(ki.wScan, 0);
        assert_eq!(ki.wVk, VIRTUAL_KEY(0));
    }
}
