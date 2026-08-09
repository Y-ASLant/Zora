use warpui::color::ColorU;

use crate::ui::color::blend::Blend;

use super::Fill;

const PHENOMENON_BACKGROUND: u32 = 0x121212FF;
const PHENOMENON_FOREGROUND: u32 = 0xFAF9F6FF;
const PHENOMENON_ACCENT: u32 = 0x2E5D9EFF;
const PHENOMENON_BLUE: u32 = 0x3780E9FF;
const PHENOMENON_BODY_TEXT: u32 = 0xFAF9F6E5;
const PHENOMENON_LABEL_TEXT: u32 = 0xFAF9F699;
const PHENOMENON_DISABLED_LABEL_TEXT: u32 = 0xFAF9F680;
const PHENOMENON_SUBTLE_BORDER: u32 = 0xFAF9F633;

pub struct PhenomenonStyle;

impl PhenomenonStyle {
    pub fn background() -> ColorU {
        ColorU::from_u32(PHENOMENON_BACKGROUND)
    }

    pub fn foreground() -> ColorU {
        ColorU::from_u32(PHENOMENON_FOREGROUND)
    }

    pub fn accent() -> ColorU {
        ColorU::from_u32(PHENOMENON_ACCENT)
    }

    pub fn blue() -> ColorU {
        ColorU::from_u32(PHENOMENON_BLUE)
    }

    pub fn body_text() -> ColorU {
        ColorU::from_u32(PHENOMENON_BODY_TEXT)
    }

    pub fn label_text() -> ColorU {
        ColorU::from_u32(PHENOMENON_LABEL_TEXT)
    }

    pub fn disabled_label_text() -> ColorU {
        ColorU::from_u32(PHENOMENON_DISABLED_LABEL_TEXT)
    }

    pub fn subtle_border() -> ColorU {
        ColorU::from_u32(PHENOMENON_SUBTLE_BORDER)
    }

    pub fn tinted_surface() -> Fill {
        Fill::Solid(Self::background()).blend(&Fill::Solid(Self::blue()).with_opacity(50))
    }

    pub fn surface_border() -> ColorU {
        Self::blue()
    }

    pub fn primary_button_background(hovered: bool) -> Fill {
        Fill::Solid(if hovered {
            Self::blue()
        } else {
            Self::accent()
        })
    }

    pub fn primary_button_text() -> ColorU {
        Self::foreground()
    }

    pub fn segmented_control_background() -> Fill {
        Fill::Solid(Self::foreground()).with_opacity(8)
    }

    pub fn selected_chip_background() -> Fill {
        Fill::Solid(Self::foreground())
    }

    pub fn selected_chip_text() -> ColorU {
        Self::background()
    }

    pub fn selected_chip_border() -> Fill {
        Fill::Solid(Self::accent())
    }

    pub fn unselected_chip_background() -> Fill {
        Fill::Solid(Self::foreground()).with_opacity(8)
    }

    pub fn unselected_chip_text() -> ColorU {
        Self::body_text()
    }
}
