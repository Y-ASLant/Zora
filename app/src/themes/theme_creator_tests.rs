use crate::util::color::OPAQUE;

use super::*;

#[test]
#[cfg(not(target_family = "wasm"))]
fn top_colors_invalid_image_test() {
    let invalid_image_path: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "assets",
        "async",
        "jpg",
        "this_doesnt_exist.jpg",
    ]
    .iter()
    .collect();

    let colors = top_colors_for_image(invalid_image_path);
    assert!(colors.is_err());
}

#[test]
fn accent_colors_contrast_test() {
    let foreground = ColorU::white();
    let background = ColorU::black();
    let accent_options = [
        ColorU::new(255, 0, 0, OPAQUE),
        ColorU::new(100, 0, 0, OPAQUE),
        ColorU::new(10, 0, 0, OPAQUE),
    ];
    assert_eq!(
        accent_options[1],
        pick_accent_color_from_options(&[background, foreground], &accent_options)
    );
}
