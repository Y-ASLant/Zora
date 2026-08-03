use super::*;

#[test]
fn numeric_font_weights_only_map_to_supported_standard_weights() {
    assert_eq!(Weight::from_numeric_weight(400), Some(Weight::Normal));
    assert_eq!(Weight::from_numeric_weight(500), Some(Weight::Medium));
    assert_eq!(Weight::from_numeric_weight(450), None);
}

#[test]
fn test_subpixel_alignment_computation() {
    {
        // Default case - a non-fractional offset should have an alignment
        // value of zero.
        let pos = vec2f(0., 0.);
        let alignment = SubpixelAlignment::new(pos);
        assert_eq!(alignment.0, 0);
    }
    {
        // 0.1 rounds down to 0 (not up to 0.33 -> 1)
        let pos = vec2f(0.1, 0.);
        let alignment = SubpixelAlignment::new(pos);
        assert_eq!(alignment.0, 0);
    }
    {
        // y-position doesn't affect computation
        let pos = vec2f(0.1, 0.33);
        let alignment = SubpixelAlignment::new(pos);
        assert_eq!(alignment.0, 0);
    }
    {
        // 0.2 rounds up to 0.33 -> 1
        let pos = vec2f(0.2, 0.);
        let alignment = SubpixelAlignment::new(pos);
        assert_eq!(alignment.0, 1);
    }
    {
        // 0.66 doesn't round, and converts to 2
        let pos = vec2f(0.66, 0.);
        let alignment = SubpixelAlignment::new(pos);
        assert_eq!(alignment.0, 2);
    }
    {
        // 0.9 rounds up to 1.0 -> 0
        let pos = vec2f(0.9, 0.);
        let alignment = SubpixelAlignment::new(pos);
        assert_eq!(alignment.0, 0);
    }
}
