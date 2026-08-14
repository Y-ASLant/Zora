//! 传输面板渲染组件
//!
//! 提供文件传输进度面板的渲染功能，包括传输方向图标、状态标签、进度条和传输列表。
//! author: logic
//! date: 2026-05-26

use warp_core::ui::appearance::Appearance;
use warpui::elements::{
    ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox, Container, CornerRadius,
    CrossAxisAlignment, Empty, Fill, Flex, Hoverable, MainAxisAlignment, MainAxisSize,
    MouseStateHandle, ParentElement, Radius, SavePosition, ScrollbarWidth, Shrinkable, Text,
};
use warpui::platform::Cursor;
use warpui::Element;

use crate::sftp_manager::browser::SftpBrowserAction;
use crate::sftp_manager::types::{format_size, TransferDirection, TransferState, TransferTask};
use crate::ui_components::icons::Icon;

/// 进度条高度
const PROGRESS_BAR_HEIGHT: f32 = 8.0;
/// 面板内边距
const PANEL_PADDING: f32 = 8.0;
/// 传输面板位置 ID
const TRANSFER_PANEL_POSITION_ID: &str = "sftp_transfer_panel";

/// 渲染传输方向图标
fn render_direction_icon(
    direction: &TransferDirection,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let theme = appearance.theme();
    let icon = match direction {
        TransferDirection::Upload => Icon::UploadCloud,
        TransferDirection::Download => Icon::Download,
    };
    let icon_color = match direction {
        TransferDirection::Upload => theme.accent(),
        TransferDirection::Download => theme.ui_green_color().into(),
    };

    ConstrainedBox::new(icon.to_warpui_icon(icon_color).finish())
        .with_width(14.0)
        .with_height(14.0)
        .finish()
}

/// 渲染传输状态标签
fn render_state_label(state: &TransferState, appearance: &Appearance) -> Box<dyn Element> {
    let theme = appearance.theme();
    let ui_font = appearance.ui_font_family();
    let ui_font_size = appearance.ui_font_size();

    let (label, color) = match state {
        TransferState::Pending => (
            String::from("等待中"),
            theme.sub_text_color(theme.background()),
        ),
        TransferState::InProgress => (String::from("传输中"), theme.accent()),
        TransferState::Paused => (
            String::from("已暂停"),
            theme.sub_text_color(theme.background()),
        ),
        TransferState::Completed => (String::from("已完成"), theme.ui_green_color().into()),
        TransferState::Failed(_) => (String::from("失败"), theme.ui_error_color().into()),
        TransferState::Cancelled => (
            String::from("已取消"),
            theme.sub_text_color(theme.background()),
        ),
    };

    Text::new_inline(label, ui_font, ui_font_size)
        .with_color(color.into())
        .finish()
}

fn transfer_failure_reason(state: &TransferState) -> Option<&str> {
    match state {
        TransferState::Failed(reason) => Some(reason),
        TransferState::Pending
        | TransferState::InProgress
        | TransferState::Paused
        | TransferState::Completed
        | TransferState::Cancelled => None,
    }
}

fn render_task_action(
    label: &str,
    task_id: usize,
    action: SftpBrowserAction,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let label = label.to_string();
    let font = appearance.ui_font_family();
    let size = appearance.ui_font_size() * 0.8;
    let text_color = appearance
        .theme()
        .sub_text_color(appearance.theme().background());
    let position_id = format!("sftp_btn:transfer_action:{task_id}:{label}");
    let element = Hoverable::new(Default::default(), move |_| {
        Container::new(
            Text::new_inline(label.clone(), font, size)
                .with_color(text_color.into())
                .finish(),
        )
        .with_uniform_padding(2.0)
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_mouse_down(move |ctx, _, _| {
        ctx.dispatch_typed_action(action.clone());
    })
    .finish();
    SavePosition::new(element, &position_id).finish()
}

/// 渲染进度条
fn render_progress_bar(progress: u8, appearance: &Appearance) -> Box<dyn Element> {
    let theme = appearance.theme();
    let progress = (progress as f32 / 100.0).clamp(0.0, 1.0);
    let filled_weight = progress.max(0.001);
    let empty_weight = (1.0 - progress).max(0.001);
    let radius = CornerRadius::with_all(Radius::Pixels(PROGRESS_BAR_HEIGHT / 2.0));

    ConstrainedBox::new(
        Container::new(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Shrinkable::new(
                        filled_weight,
                        ConstrainedBox::new(
                            Container::new(Empty::new().finish())
                                .with_background(theme.accent())
                                .with_corner_radius(radius)
                                .finish(),
                        )
                        .with_height(PROGRESS_BAR_HEIGHT)
                        .finish(),
                    )
                    .finish(),
                )
                .with_child(
                    Shrinkable::new(
                        empty_weight,
                        ConstrainedBox::new(Empty::new().finish())
                            .with_height(PROGRESS_BAR_HEIGHT)
                            .finish(),
                    )
                    .finish(),
                )
                .finish(),
        )
        .with_background(theme.surface_3())
        .with_corner_radius(radius)
        .finish(),
    )
    .with_height(PROGRESS_BAR_HEIGHT)
    .finish()
}

/// 渲染单个传输行
fn render_transfer_row(task: &TransferTask, appearance: &Appearance) -> Box<dyn Element> {
    // 方向图标
    let dir_icon = render_direction_icon(&task.direction, appearance);

    // 文件名
    let file_name = task
        .source_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let progress = task.progress_percent();
    let name_el = Text::new_inline(
        file_name,
        appearance.ui_font_family(),
        appearance.ui_font_size(),
    )
    .with_color(appearance.theme().active_ui_text_color().into())
    .finish();

    // 状态标签
    let state_el = render_state_label(&task.state, appearance);

    // 第一行：图标 + 文件名 + 百分比 + 状态 + 操作按钮
    let mut top_row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(6.0)
        .with_child(dir_icon)
        .with_child(Shrinkable::new(1.0, name_el).finish())
        .with_child(
            Text::new_inline(
                format!("{progress}%"),
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(appearance.theme().active_ui_text_color().into())
            .finish(),
        )
        .with_child(state_el);

    let task_id = task.id;
    match &task.state {
        TransferState::InProgress => {
            top_row = top_row
                .with_child(render_task_action(
                    "暂停",
                    task_id,
                    SftpBrowserAction::PauseTransfer(task_id),
                    appearance,
                ))
                .with_child(render_task_action(
                    "取消",
                    task_id,
                    SftpBrowserAction::CancelTransfer(task_id),
                    appearance,
                ));
        }
        TransferState::Paused => {
            top_row = top_row
                .with_child(render_task_action(
                    "恢复",
                    task_id,
                    SftpBrowserAction::ResumeTransfer(task_id),
                    appearance,
                ))
                .with_child(render_task_action(
                    "取消",
                    task_id,
                    SftpBrowserAction::CancelTransfer(task_id),
                    appearance,
                ));
        }
        TransferState::Failed(_) | TransferState::Cancelled => {
            top_row = top_row.with_child(render_task_action(
                "重试",
                task_id,
                SftpBrowserAction::RetryTransfer(task_id),
                appearance,
            ));
        }
        TransferState::Pending | TransferState::Completed => {}
    }

    let mut col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(4.0)
        .with_child(top_row.finish());

    if let Some(reason) = transfer_failure_reason(&task.state) {
        col.add_child(
            Shrinkable::new(
                1.0,
                Text::new_inline(
                    format!("失败: {reason}"),
                    appearance.ui_font_family(),
                    appearance.ui_font_size() * 0.8,
                )
                .with_color(appearance.theme().ui_error_color().into())
                .finish(),
            )
            .finish(),
        );
    }

    // 进度条（仅传输中显示）
    if matches!(
        task.state,
        TransferState::InProgress | TransferState::Paused
    ) {
        let bar = render_progress_bar(progress, appearance);
        col.add_child(bar);
        let mut stats = format!(
            "已传 {} / {} · 速度 {}/s",
            format_size(task.transferred),
            format_size(task.total_size),
            format_size(task.speed_bytes_per_second),
        );
        if task.total_files != 1 {
            stats = format!(
                "{stats} · {} / {} 个文件",
                task.completed_files, task.total_files,
            );
        }
        col.add_child(
            Text::new_inline(
                stats,
                appearance.ui_font_family(),
                appearance.ui_font_size() * 0.9,
            )
            .with_color(appearance.theme().active_ui_text_color().into())
            .finish(),
        );
    }

    Container::new(col.finish())
        .with_padding_top(4.0)
        .with_padding_bottom(4.0)
        .finish()
}

/// 渲染文件传输面板（主入口）
///
/// 始终显示传输任务列表，标题栏右侧包含关闭按钮。
pub fn render_transfer_panel(
    transfers: &[TransferTask],
    appearance: &Appearance,
    max_height: f32,
    scroll_state: ClippedScrollStateHandle,
    close_btn_state: MouseStateHandle,
) -> Box<dyn Element> {
    let theme = appearance.theme();
    let text_color = theme.active_ui_text_color();
    let ui_font = appearance.ui_font_family();
    let ui_font_size = appearance.ui_font_size();

    // 标题栏
    let count = transfers.len();
    let title_text = format!("传输 ({count})");

    let title_el = Text::new_inline(title_text, ui_font, ui_font_size)
        .with_color(text_color.into())
        .finish();

    // 关闭按钮
    let icon_color = theme.sub_text_color(theme.background());
    let close_btn = Hoverable::new(close_btn_state, move |_| {
        let icon_el = ConstrainedBox::new(Icon::X.to_warpui_icon(icon_color).finish())
            .with_width(12.0)
            .with_height(12.0)
            .finish();
        Container::new(icon_el)
            .with_padding_left(4.0)
            .with_padding_right(4.0)
            .with_padding_top(4.0)
            .with_padding_bottom(4.0)
            .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(|ctx, _, _| {
        ctx.dispatch_typed_action(SftpBrowserAction::ToggleTransferPanel);
    })
    .finish();

    let header = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_size(MainAxisSize::Max)
        .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
        .with_child(title_el)
        .with_child(close_btn)
        .finish();

    let mut col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(header);

    let rows_col = {
        let mut inner = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(4.0);
        for task in transfers {
            let row = render_transfer_row(task, appearance);
            inner.add_child(row);
        }
        inner.finish()
    };
    let scrollbar_color = theme.disabled_text_color(theme.background()).into();
    let scrollbar_thumb_hover = theme.main_text_color(theme.background()).into();
    let rows = ClippedScrollable::vertical(
        scroll_state,
        rows_col,
        ScrollbarWidth::Auto,
        scrollbar_color,
        scrollbar_thumb_hover,
        Fill::None,
    )
    .finish();
    col.add_child(Shrinkable::new(1.0, rows).finish());

    let panel = Container::new(col.finish())
        .with_uniform_padding(PANEL_PADDING)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .with_background(theme.surface_2())
        .finish();
    SavePosition::new(
        ConstrainedBox::new(panel)
            .with_max_height(max_height)
            .finish(),
        TRANSFER_PANEL_POSITION_ID,
    )
    .finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::cell::RefCell;
    use std::collections::HashSet;
    use std::path::PathBuf;
    use std::rc::Rc;

    use pathfinder_geometry::vector::vec2f;
    use warpui::elements::{ParentElement, Stack};
    use warpui::platform::WindowStyle;
    use warpui::{
        App, AppContext, Entity, Event, Presenter, SingletonEntity, TypedActionView, View,
        ViewContext, WindowInvalidation,
    };

    struct TransferPanelTestView {
        transfers: Vec<TransferTask>,
        close_btn_state: MouseStateHandle,
        scroll_state: ClippedScrollStateHandle,
        actions: Vec<SftpBrowserAction>,
    }

    impl TransferPanelTestView {
        /// 创建用于验证传输面板点击行为的测试视图
        fn new() -> Self {
            Self {
                transfers: vec![make_transfer_task(1)],
                close_btn_state: MouseStateHandle::default(),
                scroll_state: ClippedScrollStateHandle::default(),
                actions: Vec::new(),
            }
        }
    }

    impl Entity for TransferPanelTestView {
        type Event = ();
    }

    impl TypedActionView for TransferPanelTestView {
        type Action = SftpBrowserAction;

        /// 处理传输面板派发的测试动作
        fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
            self.actions.push(action.clone());
            ctx.notify();
        }
    }

    impl View for TransferPanelTestView {
        fn ui_name() -> &'static str {
            "TransferPanelTestView"
        }

        /// 渲染测试用传输面板
        fn render(&self, app: &AppContext) -> Box<dyn Element> {
            let appearance = Appearance::as_ref(app);
            Stack::new()
                .with_child(render_transfer_panel(
                    &self.transfers,
                    appearance,
                    200.0,
                    self.scroll_state.clone(),
                    self.close_btn_state.clone(),
                ))
                .finish()
        }
    }

    /// 初始化传输面板测试所需的外观单例
    fn initialize_app(app: &mut App) {
        app.add_singleton_model(|_| Appearance::mock());
    }

    /// 创建一个测试用传输任务
    fn make_transfer_task(id: usize) -> TransferTask {
        let mut task = TransferTask::new(
            id,
            PathBuf::from(format!("/remote/file_{id}.txt")),
            PathBuf::from(format!("/local/file_{id}.txt")),
            TransferDirection::Download,
            1024,
        );
        task.state = TransferState::InProgress;
        task
    }

    #[test]
    fn failed_transfer_preserves_failure_reason() {
        let state = TransferState::Failed(String::from("连接已断开"));

        assert_eq!(transfer_failure_reason(&state), Some("连接已断开"));
    }

    #[test]
    fn clicking_pause_and_cancel_dispatches_actions() {
        App::test((), |mut app| async move {
            initialize_app(&mut app);
            let (window_id, view) =
                app.add_window(WindowStyle::NotStealFocus, |_| TransferPanelTestView::new());
            let root_view_id = app.root_view_id(window_id).expect("测试窗口应包含根视图");
            let presenter = Rc::new(RefCell::new(Presenter::new(window_id)));
            let invalidation = WindowInvalidation {
                updated: HashSet::from([root_view_id]),
                ..Default::default()
            };

            app.update({
                let presenter = presenter.clone();
                move |ctx| {
                    presenter.borrow_mut().invalidate(invalidation, ctx);
                    presenter
                        .borrow_mut()
                        .build_scene(vec2f(800., 600.), 1., None, ctx);

                    let pause_bounds = presenter
                        .borrow()
                        .position_cache()
                        .get_position("sftp_btn:transfer_action:1:暂停")
                        .expect("暂停按钮必须出现在传输行中");
                    let cancel_bounds = presenter
                        .borrow()
                        .position_cache()
                        .get_position("sftp_btn:transfer_action:1:取消")
                        .expect("取消按钮必须出现在传输行中");

                    for position in [pause_bounds.origin(), cancel_bounds.origin()] {
                        ctx.simulate_window_event(
                            Event::LeftMouseDown {
                                position: position + vec2f(1., 1.),
                                modifiers: Default::default(),
                                click_count: 1,
                                is_first_mouse: false,
                            },
                            window_id,
                            presenter.clone(),
                        );
                        presenter.borrow_mut().invalidate(
                            WindowInvalidation {
                                updated: HashSet::from([root_view_id]),
                                ..Default::default()
                            },
                            ctx,
                        );
                        presenter
                            .borrow_mut()
                            .build_scene(vec2f(800., 600.), 1., None, ctx);
                        ctx.simulate_window_event(
                            Event::LeftMouseUp {
                                position: position + vec2f(1., 1.),
                                modifiers: Default::default(),
                            },
                            window_id,
                            presenter.clone(),
                        );
                    }
                }
            });

            view.read(&app, |view, _| {
                assert!(matches!(
                    view.actions.first(),
                    Some(SftpBrowserAction::PauseTransfer(1))
                ));
                assert!(matches!(
                    view.actions.get(1),
                    Some(SftpBrowserAction::CancelTransfer(1))
                ));
            });
        });
    }

    /// 验证点击传输面板背景区域不会影响传输内容展示
    #[test]
    fn clicking_panel_background_does_not_toggle_transfer_panel() {
        App::test((), |mut app| async move {
            initialize_app(&mut app);
            let (window_id, view) =
                app.add_window(WindowStyle::NotStealFocus, |_| TransferPanelTestView::new());
            let root_view_id = app.root_view_id(window_id).expect("测试窗口应包含根视图");
            let presenter = Rc::new(RefCell::new(Presenter::new(window_id)));
            let invalidation = WindowInvalidation {
                updated: HashSet::from([root_view_id]),
                ..Default::default()
            };

            app.update({
                let presenter = presenter.clone();
                move |ctx| {
                    presenter.borrow_mut().invalidate(invalidation, ctx);
                    presenter
                        .borrow_mut()
                        .build_scene(vec2f(320., 120.), 1., None, ctx);

                    ctx.simulate_window_event(
                        Event::LeftMouseDown {
                            position: vec2f(4., 12.),
                            modifiers: Default::default(),
                            click_count: 1,
                            is_first_mouse: false,
                        },
                        window_id,
                        presenter.clone(),
                    );
                    ctx.simulate_window_event(
                        Event::LeftMouseUp {
                            position: vec2f(4., 12.),
                            modifiers: Default::default(),
                        },
                        window_id,
                        presenter,
                    );
                }
            });

            view.read(&app, |view, _| {
                assert_eq!(
                    view.transfers.len(),
                    1,
                    "点击传输面板背景区域后传输内容应保持显示"
                );
            });
        });
    }

    /// 验证传输记录超过限制高度后，面板保持限高并支持内部滚动
    #[test]
    fn transfer_panel_is_height_limited_and_scrollable() {
        App::test((), |mut app| async move {
            initialize_app(&mut app);
            let (window_id, view) = app.add_window(WindowStyle::NotStealFocus, |_| {
                let mut view = TransferPanelTestView::new();
                view.transfers = (1..=30).map(make_transfer_task).collect();
                view
            });
            let root_view_id = app.root_view_id(window_id).expect("测试窗口应包含根视图");
            let presenter = Rc::new(RefCell::new(Presenter::new(window_id)));

            app.update({
                let presenter = presenter.clone();
                move |ctx| {
                    presenter.borrow_mut().invalidate(
                        WindowInvalidation {
                            updated: HashSet::from([root_view_id]),
                            ..Default::default()
                        },
                        ctx,
                    );
                    presenter
                        .borrow_mut()
                        .build_scene(vec2f(800.0, 600.0), 1.0, None, ctx);

                    let panel_bounds = presenter
                        .borrow()
                        .position_cache()
                        .get_position(TRANSFER_PANEL_POSITION_ID)
                        .expect("传输面板必须保存布局位置");
                    assert!(
                        panel_bounds.height() <= 200.0,
                        "面板高度不得超过窗口高度的三分之一"
                    );

                    ctx.simulate_window_event(
                        Event::ScrollWheel {
                            position: panel_bounds.origin()
                                + vec2f(panel_bounds.width() / 2.0, panel_bounds.height() / 2.0),
                            delta: vec2f(0.0, -80.0),
                            precise: true,
                            modifiers: Default::default(),
                        },
                        window_id,
                        presenter,
                    );
                }
            });

            view.read(&app, |view, _| {
                assert!(
                    view.scroll_state.scroll_start().as_f32() > 0.0,
                    "传输记录溢出后应可向下滚动"
                );
            });
        });
    }
}
