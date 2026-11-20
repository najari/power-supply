//! Minimal two-language support. Strings are written inline as `t(lang, "한국어", "English")`.

use serde::{Deserialize, Serialize};

/// UI language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Lang {
    /// Korean.
    #[default]
    Ko,
    /// English.
    En,
}

impl Lang {
    /// Name of the language in itself.
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::Ko => "한국어",
            Lang::En => "English",
        }
    }
}

/// Pick the string for `lang`.
pub fn t(lang: Lang, ko: &'static str, en: &'static str) -> &'static str {
    match lang {
        Lang::Ko => ko,
        Lang::En => en,
    }
}
