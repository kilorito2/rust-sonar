//! Idioma de la interfaz: español o inglés.
//!
//! Los textos van escritos en los dos idiomas donde se usan (`tr!("Juego", "Game")`), así cada
//! traducción está al lado de su original y no hay tablas que mantener aparte. El idioma activo es
//! un atómico global: lo fija la configuración (`Settings::language`) en el motor y en la ventana.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};

/// Texto en español e inglés: `tr!("Juego", "Game")`.
#[macro_export]
macro_rules! tr {
    ($es:expr, $en:expr $(,)?) => {
        $crate::i18n::pick($es, $en)
    };
}

/// `format!` en dos idiomas: `trf!("Hola {nombre}", "Hello {nombre}")` (admite argumentos extra).
#[macro_export]
macro_rules! trf {
    ($es:literal, $en:literal $(, $arg:expr)* $(,)?) => {
        if $crate::i18n::en() { format!($en $(, $arg)*) } else { format!($es $(, $arg)*) }
    };
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    /// El del sistema: español si Windows está en español, si no inglés.
    #[default]
    Auto,
    Spanish,
    English,
}

impl Language {
    pub const ALL: [Language; 3] = [Language::Auto, Language::Spanish, Language::English];

    /// Nombre en su propio idioma (para el selector, que debe entenderse en cualquier idioma).
    pub fn label(self) -> &'static str {
        match self {
            Language::Auto => tr!("Automático (sistema)", "Automatic (system)"),
            Language::Spanish => "Español",
            Language::English => "English",
        }
    }
}

static ENGLISH: AtomicBool = AtomicBool::new(false);

/// true si la interfaz está en inglés.
pub fn en() -> bool {
    ENGLISH.load(Ordering::Relaxed)
}

pub fn set(lang: Language) {
    // Para desarrollo y capturas: RUST_SONAR_LANG=en|es manda sobre la configuración.
    let lang = match std::env::var("RUST_SONAR_LANG").as_deref() {
        Ok("en") => Language::English,
        Ok("es") => Language::Spanish,
        _ => lang,
    };
    let english = match lang {
        Language::Auto => !system_is_spanish(),
        Language::Spanish => false,
        Language::English => true,
    };
    ENGLISH.store(english, Ordering::Relaxed);
}

/// Idioma de la interfaz de Windows (LANG_SPANISH = 0x0A en el identificador de idioma primario).
pub fn system_is_spanish() -> bool {
    let id = unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() };
    id & 0x3ff == 0x0a
}

#[inline]
pub fn pick(es: &'static str, en_text: &'static str) -> &'static str {
    if en() { en_text } else { es }
}
