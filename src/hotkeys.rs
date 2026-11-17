//! Teclas que se pueden usar en los atajos globales: su nombre (el que se guarda en la
//! configuración), su código virtual de Windows y su tecla en egui (para grabarlas desde la ventana).
//!
//! Solo teclas cuya posición no cambia entre distribuciones de teclado: letras, números, F1-F24,
//! navegación y Pausa / Bloq Despl. Los signos de puntuación se quedan fuera porque su código
//! virtual depende de la distribución (en un teclado español no están donde en uno inglés).

use crate::config::Hotkey;
use eframe::egui::Key;

/// (nombre guardado, código virtual, tecla de egui, necesita modificador)
pub struct KeyDef {
    pub name: &'static str,
    pub vk: u32,
    pub egui: Key,
    /// Sola (sin Ctrl/Alt/Mayús) taparía una tecla de escribir.
    pub needs_modifier: bool,
}

macro_rules! keys {
    ($($name:literal $vk:literal $key:ident $m:literal),* $(,)?) => {
        pub const KEYS: &[KeyDef] = &[$(KeyDef { name: $name, vk: $vk, egui: Key::$key, needs_modifier: $m }),*];
    };
}

keys! {
    "A" 0x41 A true, "B" 0x42 B true, "C" 0x43 C true, "D" 0x44 D true, "E" 0x45 E true,
    "F" 0x46 F true, "G" 0x47 G true, "H" 0x48 H true, "I" 0x49 I true, "J" 0x4A J true,
    "K" 0x4B K true, "L" 0x4C L true, "M" 0x4D M true, "N" 0x4E N true, "O" 0x4F O true,
    "P" 0x50 P true, "Q" 0x51 Q true, "R" 0x52 R true, "S" 0x53 S true, "T" 0x54 T true,
    "U" 0x55 U true, "V" 0x56 V true, "W" 0x57 W true, "X" 0x58 X true, "Y" 0x59 Y true,
    "Z" 0x5A Z true,
    "0" 0x30 Num0 true, "1" 0x31 Num1 true, "2" 0x32 Num2 true, "3" 0x33 Num3 true, "4" 0x34 Num4 true,
    "5" 0x35 Num5 true, "6" 0x36 Num6 true, "7" 0x37 Num7 true, "8" 0x38 Num8 true, "9" 0x39 Num9 true,
    "F1" 0x70 F1 false, "F2" 0x71 F2 false, "F3" 0x72 F3 false, "F4" 0x73 F4 false, "F5" 0x74 F5 false,
    "F6" 0x75 F6 false, "F7" 0x76 F7 false, "F8" 0x77 F8 false, "F9" 0x78 F9 false, "F10" 0x79 F10 false,
    "F11" 0x7A F11 false, "F12" 0x7B F12 false, "F13" 0x7C F13 false, "F14" 0x7D F14 false, "F15" 0x7E F15 false,
    "F16" 0x7F F16 false, "F17" 0x80 F17 false, "F18" 0x81 F18 false, "F19" 0x82 F19 false, "F20" 0x83 F20 false,
    "F21" 0x84 F21 false, "F22" 0x85 F22 false, "F23" 0x86 F23 false, "F24" 0x87 F24 false,
    "Space" 0x20 Space true, "PageUp" 0x21 PageUp true, "PageDown" 0x22 PageDown true,
    "End" 0x23 End true, "Home" 0x24 Home true, "Left" 0x25 ArrowLeft true, "Up" 0x26 ArrowUp true,
    "Right" 0x27 ArrowRight true, "Down" 0x28 ArrowDown true, "Insert" 0x2D Insert true,
    "Delete" 0x2E Delete true, "Pause" 0x13 Pause false, "ScrollLock" 0x91 ScrollLock false,
}

pub fn find(name: &str) -> Option<&'static KeyDef> {
    KEYS.iter().find(|k| k.name.eq_ignore_ascii_case(name))
}

/// Nombre visible de una tecla (en el idioma de la interfaz).
fn key_label(name: &str) -> &str {
    match name {
        "Space" => tr!("Espacio", "Space"),
        "PageUp" => tr!("Re Pág", "Page Up"),
        "PageDown" => tr!("Av Pág", "Page Down"),
        "End" => tr!("Fin", "End"),
        "Home" => tr!("Inicio", "Home"),
        "Left" => "←",
        "Up" => "↑",
        "Right" => "→",
        "Down" => "↓",
        "Insert" => tr!("Insert", "Insert"),
        "Delete" => tr!("Supr", "Delete"),
        "Pause" => tr!("Pausa", "Pause"),
        "ScrollLock" => tr!("Bloq Despl", "Scroll Lock"),
        other => other,
    }
}

/// «Ctrl + Alt + M».
pub fn label(h: &Hotkey) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if h.ctrl {
        parts.push("Ctrl");
    }
    if h.alt {
        parts.push("Alt");
    }
    if h.shift {
        parts.push(tr!("Mayús", "Shift"));
    }
    if h.win {
        parts.push("Win");
    }
    parts.push(key_label(&h.key));
    parts.join(" + ")
}

/// Modificadores de `RegisterHotKey` (MOD_ALT 1, MOD_CONTROL 2, MOD_SHIFT 4, MOD_WIN 8) más
/// MOD_NOREPEAT, para que mantener pulsado no lo active una y otra vez.
pub fn modifiers(h: &Hotkey) -> u32 {
    let mut m = 0x4000;
    if h.alt {
        m |= 1;
    }
    if h.ctrl {
        m |= 2;
    }
    if h.shift {
        m |= 4;
    }
    if h.win {
        m |= 8;
    }
    m
}

/// Resultado de pulsar una tecla mientras se graba un atajo.
pub enum Captured {
    Key(Hotkey),
    /// Tecla de escribir sin modificador: no se acepta.
    NeedsModifier,
    /// Tecla que no se puede usar (Esc la usa la ventana para cancelar).
    Unsupported,
}

/// Convierte una pulsación de egui en atajo. Se usa la tecla física (posición en un teclado
/// estadounidense): con Ctrl + Alt (= AltGr) la tecla «lógica» de un teclado español sería el
/// símbolo que escribe AltGr (`@`, `#`…) y no el número.
pub fn from_egui(key: Key, physical: Option<Key>, mods: eframe::egui::Modifiers) -> Captured {
    let key = physical.unwrap_or(key);
    let Some(def) = KEYS.iter().find(|k| k.egui == key) else { return Captured::Unsupported };
    let h = Hotkey { ctrl: mods.ctrl, alt: mods.alt, shift: mods.shift, win: false, key: def.name.into() };
    if def.needs_modifier && !(h.ctrl || h.alt) {
        // Mayús + letra también escribe (mayúsculas): hace falta Ctrl o Alt.
        return Captured::NeedsModifier;
    }
    Captured::Key(h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::Modifiers;

    #[test]
    fn default_hotkeys_are_known_keys() {
        let h = crate::config::Hotkeys::default();
        for t in crate::config::MuteTarget::ALL {
            if let Some(k) = h.get(t) {
                assert!(find(&k.key).is_some(), "{}", k.key);
            }
        }
    }

    #[test]
    fn capture_rules() {
        let ctrl_alt = Modifiers { ctrl: true, alt: true, ..Default::default() };
        assert!(matches!(from_egui(Key::M, None, ctrl_alt), Captured::Key(ref h) if h.key == "M" && modifiers(h) == 0x4003));
        assert!(matches!(from_egui(Key::M, None, Modifiers::SHIFT), Captured::NeedsModifier));
        assert!(matches!(from_egui(Key::F9, None, Modifiers::NONE), Captured::Key(_)));
        assert!(matches!(from_egui(Key::Escape, None, Modifiers::NONE), Captured::Unsupported));
        // AltGr + 2 en un teclado español: lógica «@», física «2».
        assert!(matches!(from_egui(Key::Questionmark, Some(Key::Num2), ctrl_alt), Captured::Key(ref h) if h.key == "2"));
    }
}
