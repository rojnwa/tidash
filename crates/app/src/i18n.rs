//! Translations: English only for now (the source language, so no catalog is needed).
//! Drop `po/<tag>.po` files in and add a `Locale` variant to ship another language.

use std::borrow::Cow;

include!(concat!(env!("OUT_DIR"), "/catalogs.rs"));

#[derive(Clone, Copy, Default)]
pub enum Locale {
    #[default]
    English,
}

impl fastframe_i18n::Locale for Locale {
    fn catalog(self) -> Option<&'static dyn fastframe_i18n::Translator> {
        match self {
            Self::English => None,
        }
    }
}

/// Looks up a user-visible string.
pub fn tr(source: &'static str) -> Cow<'static, str> {
    fastframe_i18n::gettext(Locale::default(), source)
}
