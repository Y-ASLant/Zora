use super::EditorElement;
use crate::settings::CursorDisplayType;
use pathfinder_geometry::vector::vec2f;

const FLOAT_TOLERANCE: f32 = 1e-4;

#[test]
fn scroll_position_y_fract_is_continuous_at_first_visible_row_boundary_without_top_section() {
    let line_height = 20.0;

    let fract_before_boundary = EditorElement::scroll_position_y_fract(0.99, line_height, 0.0);
    assert!((fract_before_boundary - 19.8).abs() < FLOAT_TOLERANCE);

    let fract_at_boundary = EditorElement::scroll_position_y_fract(1.0, line_height, 0.0);
    assert!((fract_at_boundary - 0.0).abs() < FLOAT_TOLERANCE);
}

#[test]
fn scroll_position_y_fract_is_continuous_at_first_visible_row_boundary_with_top_section() {
    let line_height = 20.0;
    let top_section_height_px = 10.0;

    let fract_before_boundary =
        EditorElement::scroll_position_y_fract(1.49, line_height, top_section_height_px);
    assert!((fract_before_boundary - 29.8).abs() < FLOAT_TOLERANCE);

    let fract_at_boundary =
        EditorElement::scroll_position_y_fract(1.5, line_height, top_section_height_px);
    assert!((fract_at_boundary - 0.0).abs() < FLOAT_TOLERANCE);
}

#[test]
fn scroll_position_y_fract_tracks_fractional_offset_after_boundary() {
    let line_height = 20.0;
    let top_section_height_px = 10.0;

    let fract = EditorElement::scroll_position_y_fract(1.75, line_height, top_section_height_px);
    assert!((fract - 5.0).abs() < FLOAT_TOLERANCE);
}

#[test]
fn underline_cursor_is_drawn_at_the_bottom_of_the_cursor_area() {
    let origin = vec2f(4., 8.);
    let rect = EditorElement::cursor_rect(CursorDisplayType::Underline, origin, 10., 16.8, 14.);

    assert!((rect.origin().y() - 22.).abs() < FLOAT_TOLERANCE);
    assert!((rect.size().y() - 2.8).abs() < FLOAT_TOLERANCE);
    assert!((rect.origin().y() + rect.size().y() - (origin.y() + 16.8)).abs() < FLOAT_TOLERANCE);
}

#[test]
fn underline_cursor_keeps_a_visible_thickness_with_compact_line_height() {
    let origin = vec2f(4., 8.);
    let rect = EditorElement::cursor_rect(CursorDisplayType::Underline, origin, 10., 10., 14.);

    assert!((rect.size().y() - 1.).abs() < FLOAT_TOLERANCE);
    assert!((rect.origin().y() + rect.size().y() - (origin.y() + 10.)).abs() < FLOAT_TOLERANCE);
}
