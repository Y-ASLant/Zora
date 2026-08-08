use super::*;

use warp_core::ui::color::{
    blend::Blend,
    contrast::{high_enough_contrast, MinimumAllowedContrast},
};

#[test]
fn cli_agent_visibility_chip_is_readable_in_every_state() {
    let appearance = Appearance::mock();

    for is_enabled in [false, true] {
        for is_clickable in [false, true] {
            for is_hovered in [false, true] {
                let (background, border_fill, text_color) = cli_agent_visibility_chip_style(
                    is_enabled,
                    is_clickable,
                    is_hovered,
                    &appearance,
                );
                let row_background = appearance.theme().surface_1().into_solid();
                let visible_background = row_background.blend(&background.into_solid());
                let visible_text = visible_background.blend(&text_color);
                assert!(
                    high_enough_contrast(
                        visible_text,
                        visible_background,
                        MinimumAllowedContrast::Text,
                    ),
                    "CLI agent 选项文字在 enabled={is_enabled}, clickable={is_clickable}, hovered={is_hovered} 状态下不可读",
                );

                let Fill::Solid(border_color) = border_fill else {
                    panic!("CLI agent 选项边框必须使用纯色");
                };
                let visible_border = visible_background.blend(&border_color);
                assert!(
                    high_enough_contrast(
                        visible_border,
                        visible_background,
                        MinimumAllowedContrast::NonText,
                    ),
                    "CLI agent 选项边框在 enabled={is_enabled}, clickable={is_clickable}, hovered={is_hovered} 状态下不可辨",
                );
            }
        }
    }
}

#[test]
fn cli_agent_visibility_chip_hover_only_changes_clickable_options() {
    let appearance = Appearance::mock();

    for is_enabled in [false, true] {
        let clickable_default =
            cli_agent_visibility_chip_style(is_enabled, true, false, &appearance)
                .0
                .into_solid();
        let clickable_hovered =
            cli_agent_visibility_chip_style(is_enabled, true, true, &appearance)
                .0
                .into_solid();
        assert_ne!(clickable_default, clickable_hovered);

        let disabled_default =
            cli_agent_visibility_chip_style(is_enabled, false, false, &appearance)
                .0
                .into_solid();
        let disabled_hovered =
            cli_agent_visibility_chip_style(is_enabled, false, true, &appearance)
                .0
                .into_solid();
        assert_eq!(disabled_default, disabled_hovered);
    }
}

#[test]
fn cli_agent_visibility_chip_keeps_selected_and_unselected_states_distinct() {
    let appearance = Appearance::mock();

    for is_clickable in [false, true] {
        let unselected = cli_agent_visibility_chip_style(false, is_clickable, false, &appearance)
            .0
            .into_solid();
        let selected = cli_agent_visibility_chip_style(true, is_clickable, false, &appearance)
            .0
            .into_solid();
        assert_ne!(unselected, selected);
    }
}
