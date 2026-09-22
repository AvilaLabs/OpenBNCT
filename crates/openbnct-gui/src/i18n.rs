// SPDX-License-Identifier: MIT

//! UI language selection — English is the authoring language, Japanese
//! the first localization target. Strings stay inline at the call site
//! via [`t!`] so a translation never drifts from the code it labels;
//! anything without a `ja` literal simply renders English.

/// The workbench's UI languages — chosen for where BNCT research
/// actually happens: English (lingua franca), Japanese (JCDS-II/KURNS
/// ecosystem), Italian (Pavia/INFN prompt-gamma groups), Simplified
/// Chinese (AB-BNCT programs), Spanish (CNEA/Bariloche).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    English,
    Japanese,
    Italian,
    ChineseSimplified,
    Spanish,
}

impl Language {
    pub const ALL: [Self; 5] = [
        Self::English,
        Self::Japanese,
        Self::Italian,
        Self::ChineseSimplified,
        Self::Spanish,
    ];

    /// Short locale tag used by `t!` arm names and `localStorage`.
    #[must_use]
    pub fn tag(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::Japanese => "ja",
            Self::Italian => "it",
            Self::ChineseSimplified => "zh",
            Self::Spanish => "es",
        }
    }

    /// Native name shown in the language picker.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::Japanese => "日本語",
            Self::Italian => "Italiano",
            Self::ChineseSimplified => "中文 (简体)",
            Self::Spanish => "Español",
        }
    }

    /// Map a locale tag (`ja-JP`, `en_US.UTF-8`, `zh-CN`, …) to a
    /// language; unknown or empty values default to English.
    #[must_use]
    pub fn from_locale(locale: &str) -> Self {
        let l = locale.to_ascii_lowercase();
        if l.starts_with("ja") {
            Self::Japanese
        } else if l.starts_with("it") {
            Self::Italian
        } else if l.starts_with("zh") {
            Self::ChineseSimplified
        } else if l.starts_with("es") {
            Self::Spanish
        } else {
            Self::English
        }
    }

    /// Persist the View-menu choice — `localStorage` on the web so a
    /// reload keeps the language; a no-op on native (which has env
    /// configuration and no canonical settings file to write yet).
    pub fn persist(self) {
        #[cfg(target_arch = "wasm32")]
        {
            let key = self.tag();
            if let Some(storage) =
                web_sys::window().and_then(|window| window.local_storage().ok().flatten())
            {
                let _ = storage.set_item("openbnct-lang", key);
            }
        }
    }

    /// Detect the preferred language: `OPENBNCT_LANG` env var beats
    /// `?lang=` URL param (web) which beats a persisted View-menu choice
    /// (web localStorage) which beats `navigator.language`/`LANG`.
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
                            return Self::from_locale(value);
                        }
                    }
                }
                if let Some(saved) = window
                    .local_storage()
                    .ok()
                    .flatten()
                    .and_then(|storage| storage.get_item("openbnct-lang").ok().flatten())
                {
                    return Self::from_locale(&saved);
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

/// Pick the localized literal — `t!(lang, en = "English", ja = "日本語",
/// it = "…", zh = "…", es = "…")`. Every arm is a named language tag;
/// `en` is the fallback when the active language has no arm. Sites with
/// dynamic content take `format!` on the picked template instead.
#[macro_export]
macro_rules! t {
    ($lang:expr, $($l:ident = $v:expr),+ $(,)?) => {{
        let tag = $crate::i18n::Language::tag($lang);
        let mut picked: &str = "";
        $(if tag == stringify!($l) {
            picked = $v;
        })+
        if picked.is_empty() {
            $(if stringify!($l) == "en" {
                picked = $v;
            })+
        }
        picked
    }};
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
        assert_eq!(crate::t!(Language::English, en = "a", ja = "あ"), "a");
        assert_eq!(crate::t!(Language::Japanese, en = "a", ja = "あ"), "あ");
        // Languages without an arm fall back to English.
        assert_eq!(crate::t!(Language::Italian, en = "a", ja = "あ"), "a");
        assert_eq!(crate::t!(Language::Spanish, en = "a", ja = "あ"), "a");
        assert_eq!(
            crate::t!(Language::ChineseSimplified, en = "a", ja = "あ", zh = "测"),
            "测"
        );
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
