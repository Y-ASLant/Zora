use crate::settings_view::SettingsSection;

use super::settings_section_for_simple_subpage;

#[test]
fn simple_settings_deeplinks_target_existing_sections() {
    assert_eq!(
        settings_section_for_simple_subpage("appearance"),
        Some(SettingsSection::Appearance)
    );
    assert_eq!(
        settings_section_for_simple_subpage("code"),
        Some(SettingsSection::Code)
    );
    assert_eq!(
        settings_section_for_simple_subpage("keyboard_shortcuts"),
        Some(SettingsSection::Keybindings)
    );
    assert_eq!(
        settings_section_for_simple_subpage("agent_profiles"),
        Some(SettingsSection::AgentProfiles)
    );
    assert_eq!(settings_section_for_simple_subpage("missing"), None);
}
