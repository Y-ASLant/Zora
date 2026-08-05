use super::*;
use pathfinder_color::ColorU;

fn relative_luminance(color: ColorU) -> f32 {
    fn linearize(channel: u8) -> f32 {
        let channel = channel as f32 / 255.0;
        if channel <= 0.03928 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    }

    0.2126 * linearize(color.r) + 0.7152 * linearize(color.g) + 0.0722 * linearize(color.b)
}

fn contrast_ratio(foreground: ColorU, background: ColorU) -> f32 {
    let foreground_luminance = relative_luminance(foreground);
    let background_luminance = relative_luminance(background);
    let (lighter, darker) = if foreground_luminance > background_luminance {
        (foreground_luminance, background_luminance)
    } else {
        (background_luminance, foreground_luminance)
    };
    (lighter + 0.05) / (darker + 0.05)
}

fn ansi_foregrounds(theme: &WarpTheme) -> [ColorU; 16] {
    let colors = theme.terminal_colors();
    [
        colors.normal.black.into(),
        colors.normal.red.into(),
        colors.normal.green.into(),
        colors.normal.yellow.into(),
        colors.normal.blue.into(),
        colors.normal.magenta.into(),
        colors.normal.cyan.into(),
        colors.normal.white.into(),
        colors.bright.black.into(),
        colors.bright.red.into(),
        colors.bright.green.into(),
        colors.bright.yellow.into(),
        colors.bright.blue.into(),
        colors.bright.magenta.into(),
        colors.bright.cyan.into(),
        colors.bright.white.into(),
    ]
}

#[test]
fn light_themes_have_readable_terminal_ansi_colors() {
    for theme in [light_theme(), gruvbox_light(), marble()] {
        let background = theme.background().into_solid();
        for (index, foreground) in ansi_foregrounds(&theme).into_iter().enumerate() {
            assert!(
                contrast_ratio(foreground, background) >= 4.5,
                "{} ANSI color {index} has insufficient contrast: {foreground:?} on {background:?}",
                theme.name().unwrap_or_default()
            );
        }
    }
}
