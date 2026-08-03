use super::*;
use crate::test_util::settings::initialize_settings_for_tests;
use crate::terminal::resizable_data::DEFAULT_VERTICAL_TABS_PANEL_WIDTH;
use settings::Setting;
use warpui::{App, SingletonEntity};

#[test]
fn use_latest_user_prompt_as_conversation_title_in_tab_names_defaults_to_false() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);

        TabSettings::handle(&app).read(&app, |settings, _ctx| {
            assert!(!*settings.use_latest_user_prompt_as_conversation_title_in_tab_names);
        });
    });
}

#[test]
fn use_latest_user_prompt_as_conversation_title_in_tab_names_uses_vertical_tabs_path() {
    assert_eq!(
        UseLatestUserPromptAsConversationTitleInTabNames::toml_path(),
        Some("appearance.vertical_tabs.use_latest_prompt_as_title")
    );
    assert_eq!(
        UseLatestUserPromptAsConversationTitleInTabNames::hierarchy(),
        Some("appearance.vertical_tabs")
    );
    assert_eq!(
        UseLatestUserPromptAsConversationTitleInTabNames::toml_key(),
        "use_latest_prompt_as_title"
    );
}

#[test]
fn show_vertical_tab_panel_in_restored_windows_defaults_to_false() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);

        TabSettings::handle(&app).read(&app, |settings, _ctx| {
            assert!(!*settings.show_vertical_tab_panel_in_restored_windows);
        });
    });
}

#[test]
fn show_vertical_tab_panel_in_restored_windows_uses_vertical_tabs_path() {
    assert_eq!(
        ShowVerticalTabPanelInRestoredWindows::toml_path(),
        Some("appearance.vertical_tabs.show_panel_in_restored_windows")
    );
    assert_eq!(
        ShowVerticalTabPanelInRestoredWindows::hierarchy(),
        Some("appearance.vertical_tabs")
    );
    assert_eq!(
        ShowVerticalTabPanelInRestoredWindows::toml_key(),
        "show_panel_in_restored_windows"
    );
}

#[test]
fn persist_vertical_tabs_panel_width_defaults_to_false() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);

        TabSettings::handle(&app).read(&app, |settings, _ctx| {
            assert!(!*settings.persist_vertical_tabs_panel_width);
        });
    });
}

#[test]
fn persist_vertical_tabs_panel_width_uses_vertical_tabs_path() {
    assert_eq!(
        PersistVerticalTabsPanelWidth::toml_path(),
        Some("appearance.vertical_tabs.persist_panel_width")
    );
    assert_eq!(
        PersistVerticalTabsPanelWidth::hierarchy(),
        Some("appearance.vertical_tabs")
    );
    assert_eq!(
        PersistVerticalTabsPanelWidth::toml_key(),
        "persist_panel_width"
    );
}

#[test]
fn remembered_vertical_tabs_panel_width_defaults_to_panel_default() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);

        TabSettings::handle(&app).read(&app, |settings, _ctx| {
            assert_eq!(
                *settings.remembered_vertical_tabs_panel_width,
                DEFAULT_VERTICAL_TABS_PANEL_WIDTH
            );
        });
    });
}
