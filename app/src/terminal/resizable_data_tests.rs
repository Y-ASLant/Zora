use super::{ModalSizes, DEFAULT_VERTICAL_TABS_PANEL_WIDTH};
use crate::app_state::WindowSnapshot;

fn window_snapshot(vertical_tabs_panel_width: Option<f32>) -> WindowSnapshot {
    WindowSnapshot {
        tabs: Vec::new(),
        active_tab_index: 0,
        bounds: None,
        fullscreen_state: Default::default(),
        quake_mode: false,
        universal_search_width: None,
        warp_ai_width: None,
        voltron_width: None,
        warp_drive_index_width: None,
        left_panel_open: false,
        vertical_tabs_panel_open: false,
        vertical_tabs_panel_width,
        left_panel_width: None,
        right_panel_width: None,
        agent_management_filters: None,
        theme_override: None,
    }
}

#[test]
fn restored_vertical_tabs_panel_width_uses_saved_value() {
    let modal_sizes = ModalSizes::from_restored(&window_snapshot(Some(376.)), 240., 480.);

    assert_eq!(
        modal_sizes
            .vertical_tabs_panel_width
            .lock()
            .expect("vertical tabs panel width handle should not be poisoned")
            .size(),
        376.
    );
}

#[test]
fn restored_vertical_tabs_panel_width_falls_back_to_default() {
    let modal_sizes = ModalSizes::from_restored(&window_snapshot(None), 240., 480.);

    assert_eq!(
        modal_sizes
            .vertical_tabs_panel_width
            .lock()
            .expect("vertical tabs panel width handle should not be poisoned")
            .size(),
        DEFAULT_VERTICAL_TABS_PANEL_WIDTH
    );
}
