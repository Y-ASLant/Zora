use super::{default_ui_font_label, fallback_font_dropdown_should_include_font, FontType};
use crate::settings::{
    MonospaceFallbackFontName, DEFAULT_MONOSPACE_FONT_NAME, DEFAULT_UI_FONT_FAMILY_NAME,
};
use settings::Setting as _;

#[test]
fn fallback_font_dropdown_includes_default_monospace_font() {
    assert_eq!(MonospaceFallbackFontName::default_value(), "");
    assert!(fallback_font_dropdown_should_include_font(
        DEFAULT_MONOSPACE_FONT_NAME,
        FontType::Monospace,
        FontType::Monospace,
        "",
    ));
}

#[test]
fn ui_font_default_label_includes_builtin_font_name() {
    assert_eq!(
        default_ui_font_label(),
        format!("{DEFAULT_UI_FONT_FAMILY_NAME} (default)")
    );
}
