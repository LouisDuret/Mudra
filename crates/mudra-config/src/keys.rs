//! Parseur de combinaisons de touches (« Ctrl+Alt+T », « Win+Shift+S », touches
//! multimédia). Partagé entre la validation de la configuration et l'exécuteur
//! d'actions (§7.8). Les codes sont des *virtual-key codes* Windows.

use std::fmt;

/// Une touche à envoyer, identifiée par son code virtuel Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key {
    pub vk: u16,
    /// Touche « étendue » (flèches, Inser, Suppr, Win, multimédia…) : nécessite
    /// `KEYEVENTF_EXTENDEDKEY` quand elle est envoyée par scancode.
    pub extended: bool,
    pub modifier: bool,
}

/// Combinaison : les modificateurs d'abord, puis les touches, dans l'ordre d'appui.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyCombo {
    pub keys: Vec<Key>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyParseError(pub String);

impl fmt::Display for KeyParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for KeyParseError {}

pub const VK_VOLUME_MUTE: u16 = 0xAD;
pub const VK_VOLUME_DOWN: u16 = 0xAE;
pub const VK_VOLUME_UP: u16 = 0xAF;
pub const VK_MEDIA_NEXT_TRACK: u16 = 0xB0;
pub const VK_MEDIA_PREV_TRACK: u16 = 0xB1;
pub const VK_MEDIA_STOP: u16 = 0xB2;
pub const VK_MEDIA_PLAY_PAUSE: u16 = 0xB3;

const fn key(vk: u16) -> Key {
    Key {
        vk,
        extended: false,
        modifier: false,
    }
}

const fn ext(vk: u16) -> Key {
    Key {
        vk,
        extended: true,
        modifier: false,
    }
}

const fn modifier(vk: u16, extended: bool) -> Key {
    Key {
        vk,
        extended,
        modifier: true,
    }
}

fn lookup(token: &str) -> Option<Key> {
    let t = token.to_ascii_lowercase();
    let k = match t.as_str() {
        "ctrl" | "control" => modifier(0x11, false),
        "alt" => modifier(0x12, false),
        "shift" | "maj" => modifier(0x10, false),
        "win" | "super" | "meta" | "windows" => modifier(0x5B, true),
        "esc" | "escape" | "echap" => key(0x1B),
        "tab" => key(0x09),
        "enter" | "return" | "entree" => key(0x0D),
        "space" | "espace" => key(0x20),
        "backspace" => key(0x08),
        "delete" | "del" | "suppr" => ext(0x2E),
        "insert" | "ins" => ext(0x2D),
        "home" => ext(0x24),
        "end" => ext(0x23),
        "pageup" | "pgup" => ext(0x21),
        "pagedown" | "pgdn" => ext(0x22),
        "left" => ext(0x25),
        "up" => ext(0x26),
        "right" => ext(0x27),
        "down" => ext(0x28),
        "printscreen" | "prtsc" => ext(0x2C),
        "pause" => key(0x13),
        "capslock" => key(0x14),
        "apps" | "menu" => ext(0x5D),
        "volumeup" | "volume_up" => ext(VK_VOLUME_UP),
        "volumedown" | "volume_down" => ext(VK_VOLUME_DOWN),
        "volumemute" | "volume_mute" | "mute" => ext(VK_VOLUME_MUTE),
        "mediaplaypause" | "playpause" | "play_pause" => ext(VK_MEDIA_PLAY_PAUSE),
        "medianext" | "next" | "nexttrack" => ext(VK_MEDIA_NEXT_TRACK),
        "mediaprev" | "prev" | "previous" | "prevtrack" => ext(VK_MEDIA_PREV_TRACK),
        "mediastop" | "stop" => ext(VK_MEDIA_STOP),
        "plus" => key(0xBB),
        "minus" => key(0xBD),
        "comma" => key(0xBC),
        "period" | "dot" => key(0xBE),
        _ => {
            let b = t.as_bytes();
            if b.len() == 1 && b[0].is_ascii_alphabetic() {
                key(b[0].to_ascii_uppercase() as u16)
            } else if b.len() == 1 && b[0].is_ascii_digit() {
                key(b[0] as u16)
            } else {
                let n = t.strip_prefix('f').and_then(|n| n.parse::<u16>().ok())?;
                if !(1..=24).contains(&n) {
                    return None;
                }
                key(0x70 + n - 1)
            }
        }
    };
    Some(k)
}

impl KeyCombo {
    /// Analyse « Ctrl+Shift+Esc », « Win+D », « MediaNext »…
    pub fn parse(s: &str) -> Result<Self, KeyParseError> {
        let mut modifiers = Vec::new();
        let mut keys = Vec::new();
        if s.trim().is_empty() {
            return Err(KeyParseError("combinaison vide".into()));
        }
        for token in s.split('+') {
            let token = token.trim();
            if token.is_empty() {
                return Err(KeyParseError(format!("touche manquante dans « {s} »")));
            }
            let k = lookup(token).ok_or_else(|| {
                KeyParseError(format!("touche inconnue « {token} » dans « {s} »"))
            })?;
            let list = if k.modifier {
                &mut modifiers
            } else {
                &mut keys
            };
            if list.contains(&k) {
                return Err(KeyParseError(format!(
                    "touche « {token} » répétée dans « {s} »"
                )));
            }
            list.push(k);
        }
        if keys.is_empty() {
            return Err(KeyParseError(format!(
                "« {s} » ne contient que des modificateurs"
            )));
        }
        modifiers.extend(keys);
        Ok(Self { keys: modifiers })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_spec_examples() {
        let c = KeyCombo::parse("Ctrl+Alt+T").unwrap();
        assert_eq!(
            c.keys.iter().map(|k| k.vk).collect::<Vec<_>>(),
            vec![0x11, 0x12, b'T' as u16]
        );
        let c = KeyCombo::parse("Win+Shift+S").unwrap();
        assert_eq!(
            c.keys.iter().map(|k| k.vk).collect::<Vec<_>>(),
            vec![0x5B, 0x10, b'S' as u16]
        );
        assert!(c.keys[0].extended);
        let c = KeyCombo::parse("Ctrl+Shift+Esc").unwrap();
        assert_eq!(c.keys.last().unwrap().vk, 0x1B);
        let c = KeyCombo::parse("Alt+Tab").unwrap();
        assert_eq!(c.keys.len(), 2);
    }

    #[test]
    fn modifiers_are_pressed_first() {
        let c = KeyCombo::parse("D+Win").unwrap();
        assert_eq!(c.keys[0].vk, 0x5B);
        assert_eq!(c.keys[1].vk, b'D' as u16);
    }

    #[test]
    fn media_and_function_keys() {
        assert_eq!(
            KeyCombo::parse("MediaNext").unwrap().keys[0].vk,
            VK_MEDIA_NEXT_TRACK
        );
        assert_eq!(KeyCombo::parse("f5").unwrap().keys[0].vk, 0x74);
        assert_eq!(KeyCombo::parse("F24").unwrap().keys[0].vk, 0x87);
    }

    #[test]
    fn rejects_garbage() {
        assert!(KeyCombo::parse("").is_err());
        assert!(KeyCombo::parse("Ctrl+").is_err());
        assert!(KeyCombo::parse("Ctrl+Alt").is_err());
        assert!(KeyCombo::parse("Ctrl+Bogus").is_err());
        assert!(KeyCombo::parse("F25").is_err());
        assert!(KeyCombo::parse("A+A").is_err());
    }
}
