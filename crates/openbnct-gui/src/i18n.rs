// SPDX-License-Identifier: Apache-2.0

//! UI language selection — English is the authoring language, Japanese
//! the first localization target. Strings stay inline at the call site
//! via [`t!`] so a translation never drifts from the code it labels;
//! anything without a `ja` literal simply renders English.

/// The workbench's UI languages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    English,
    Japanese,
}

impl Language {
    pub const ALL: [Self; 2] = [Self::English, Self::Japanese];

    /// Native name shown in the language picker.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::Japanese => "日本語",
        }
    }

    /// Map a locale tag (`ja-JP`, `en_US.UTF-8`, …) to a language;
    /// unknown or empty values default to English.
    #[must_use]
    pub fn from_locale(locale: &str) -> Self {
        if locale.to_ascii_lowercase().starts_with("ja") {
            Self::Japanese
        } else {
            Self::English
        }
    }

    /// Detect the preferred language: `OPENBNCT_LANG` env var beats
    /// `?lang=` URL param (web) which beats `navigator.language`/`LANG`.
    #[must_use]
    pub fn detect() -> Self {
        if let Ok(explicit) = std::env::var("OPENBNCT_LANG")
            && !explicit.is_empty()
        {
            return Self::from_locale(&explicit);
        }
        #[cfg(target_arch = "wasm32")]
        {
            if let Some(window) = web_sys::window() {
                if let Ok(search) = window.location().search() {
                    for pair in search.trim_start_matches('?').split('&') {
                        if let Some(value) = pair.strip_prefix("lang=") {
                            return if value.starts_with("ja") {
                                Self::Japanese
                            } else {
                                Self::English
                            };
                        }
                    }
                }
                if let Some(nav) = window.navigator().language() {
                    return Self::from_locale(&nav);
                }
            }
            Self::English
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            Self::from_locale(&std::env::var("LANG").unwrap_or_default())
        }
    }
}

/// Pick the localized literal — `t!(lang, "English", "日本語")`. Sites
/// with dynamic content take `format!` on the picked template instead.
#[macro_export]
macro_rules! t {
    ($lang:expr, $en:expr, $ja:expr) => {
        match $lang {
            $crate::i18n::Language::Japanese => $ja,
            _ => $en,
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_tags_map_to_languages() {
        assert_eq!(Language::from_locale("ja"), Language::Japanese);
        assert_eq!(Language::from_locale("ja-JP"), Language::Japanese);
        assert_eq!(Language::from_locale("ja_JP.UTF-8"), Language::Japanese);
        assert_eq!(Language::from_locale("JA"), Language::Japanese);
        assert_eq!(Language::from_locale("en_US.UTF-8"), Language::English);
        assert_eq!(Language::from_locale("fr_FR.UTF-8"), Language::English);
        assert_eq!(Language::from_locale(""), Language::English);
        assert_eq!(Language::from_locale("C"), Language::English);
    }

    #[test]
    fn t_macro_dispatches_on_language() {
        assert_eq!(crate::t!(Language::English, "a", "あ"), "a");
        assert_eq!(crate::t!(Language::Japanese, "a", "あ"), "あ");
    }

    #[test]
    fn every_language_has_a_nonempty_native_label() {
        for language in Language::ALL {
            assert!(!language.label().is_empty());
        }
    }

    #[test]
    fn cjk_fallback_font_is_embedded() {
        // The embedded OTF must parse as an SFNT container — catches a
        // truncated/corrupt asset without needing a renderer.
        let bytes = include_bytes!("../assets/NotoSansCJKjp-UI.otf");
        assert_eq!(&bytes[..4], b"OTTO");
        assert!(bytes.len() > 50_000);
    }
}
