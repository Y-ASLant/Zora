use settings::Setting as _;

use super::{CursorDisplayState, CursorDisplayType};

#[test]
fn cursor_display_defaults_are_scoped_to_settings() {
    assert_eq!(CursorDisplayType::default(), CursorDisplayType::Bar);
    assert_eq!(
        CursorDisplayState::default_value(),
        CursorDisplayType::Block
    );
}
