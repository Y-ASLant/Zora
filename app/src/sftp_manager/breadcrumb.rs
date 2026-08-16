//! 面包屑导航渲染组件
//!
//! 根据当前路径渲染可点击的面包屑导航，支持逐段导航到上级路径。
//! author: logic
//! date: 2026-05-26

use std::path::{Component, PathBuf};

use warp_core::ui::appearance::Appearance;
use warpui::elements::{
    ConstrainedBox, Container, CornerRadius, Hoverable, Radius, SavePosition, Text,
};
use warpui::platform::Cursor;
use warpui::Element;

use crate::sftp_manager::browser::SftpBrowserAction;
use crate::ui_components::icons::Icon;

/// 渲染路径面包屑导航
///
/// 遍历路径的各个组件，每段可点击触发 NavigateTo 动作。
/// 段之间用 ChevronRight 图标分隔，空路径显示 "/"。
pub fn render_breadcrumb(current_path: &PathBuf, appearance: &Appearance) -> Vec<Box<dyn Element>> {
    let theme = appearance.theme();
    let text_color = theme.active_ui_text_color();
    let sub_color = theme.sub_text_color(theme.background());
    let ui_font = appearance.ui_font_family();
    let ui_font_size = appearance.ui_font_size();

    let components: Vec<_> = current_path
        .components()
        .filter(|c| !matches!(c, Component::RootDir))
        .collect();

    // 空路径或只有根路径时只显示 "/"
    if components.is_empty() {
        let root_el = Text::new_inline(String::from("/"), ui_font, ui_font_size)
            .with_color(text_color.into())
            .finish();
        return vec![Container::new(root_el).finish()];
    }

    let mut elements: Vec<Box<dyn Element>> = Vec::new();
    let mut accumulated = PathBuf::new();

    for (i, comp) in components.iter().enumerate() {
        accumulated.push(comp);
        let is_last = i == components.len() - 1;

        // 分隔符（第一段之后添加）
        if i > 0 {
            let sep_icon =
                ConstrainedBox::new(Icon::ChevronRight.to_warpui_icon(sub_color.into()).finish())
                    .with_width(12.0)
                    .with_height(12.0)
                    .finish();
            elements.push(
                Container::new(sep_icon)
                    .with_padding_left(2.0)
                    .with_padding_right(2.0)
                    .finish(),
            );
        }

        let segment_label = comp.as_os_str().to_string_lossy().to_string();
        let target_path = accumulated.clone();

        if is_last {
            // 最后一段用高亮色，不可点击
            let text_el = Text::new_inline(segment_label, ui_font, ui_font_size)
                .with_color(text_color.into())
                .finish();
            elements.push(Container::new(text_el).finish());
        } else {
            // 非最后一段可点击导航
            let label_for_closure = segment_label.clone();
            let path = accumulated.display();
            let position_id = format!("sftp_breadcrumb:{path}");
            let hover_text_color = text_color;
            let hover_bg = theme.surface_2();
            let pressed_bg = theme.surface_3();
            let hoverable = Hoverable::new(Default::default(), move |state| {
                let (color, background) = if state.is_clicked() {
                    (hover_text_color, Some(pressed_bg))
                } else if state.is_hovered() {
                    (hover_text_color, Some(hover_bg))
                } else {
                    (sub_color, None)
                };
                let text_el = Text::new_inline(label_for_closure.clone(), ui_font, ui_font_size)
                    .with_color(color.into())
                    .finish();
                let mut container = Container::new(text_el)
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)));
                if let Some(background) = background {
                    container = container.with_background(background);
                }
                container.finish()
            })
            .with_cursor(Cursor::PointingHand)
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action(SftpBrowserAction::NavigateTo(target_path.clone()));
            })
            .finish();
            elements.push(SavePosition::new(hoverable, &position_id).finish());
        }
    }

    elements
}
