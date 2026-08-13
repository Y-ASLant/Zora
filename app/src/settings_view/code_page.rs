//! Code 设置页:Zora 在 LSP 全栈 + 持久化 workspace 历史下线后,
//! 这个页面只剩「编辑器与代码评审」相关的几个本地开关。
//!
//! 历史上这里还承载 LSP 管理子页 + codebase indexing,但都已下线;
//! `Code` 在侧边栏不再是 umbrella(没有第二个子页可挂),改为单层 Page。
//! 页面渲染的就是这一组开关本身。

#[cfg(feature = "local_fs")]
use super::features::external_editor::ExternalEditorView;
use super::{
    settings_page::{
        render_body_item, MatchData, PageType, SettingsPageMeta, SettingsPageViewHandle,
        SettingsWidget,
    },
    LocalOnlyIconState, SettingsAction, SettingsSection, ToggleState,
};
use crate::{
    appearance::Appearance,
    editor::{EditorView, Event as EditorEvent, SingleLineEditorOptions, TextOptions},
    send_telemetry_from_ctx,
    settings::{
        CodeSettings, REMOTE_FILE_AUTO_OPEN_TEXT_MAX_MIB, REMOTE_FILE_AUTO_OPEN_TEXT_MIN_MIB,
        REMOTE_FILE_LARGE_PREVIEW_MAX_KIB, REMOTE_FILE_LARGE_PREVIEW_MIN_KIB,
        REMOTE_FILE_TEXT_CACHE_MAX_MIB, REMOTE_FILE_TEXT_CACHE_MIN_MIB,
    },
    terminal::general_settings::GeneralSettings,
    workspace::tab_settings::TabSettings,
    TelemetryEvent,
};
use ai::project_context::model::{ProjectContextModel, ProjectContextModelEvent};

use settings::Setting as _;
use std::path::PathBuf;
use warp_core::{features::FeatureFlag, report_if_error, settings::ToggleableSetting as _};
use warpui::ModelContext;
use warpui::{
    elements::{ChildView, Dismiss, Element, Empty},
    keymap::ContextPredicate,
    ui_components::{
        components::{Coords, UiComponent, UiComponentStyles},
        switch::SwitchStateHandle,
    },
    Action, AppContext, Entity, SingletonEntity, TypedActionView, View, ViewContext, ViewHandle,
};

const REMOTE_FILE_NUMBER_INPUT_WIDTH: f32 = 96.;

pub struct CodeSettingsPageView {
    page: PageType<Self>,
    #[cfg(feature = "local_fs")]
    external_editor_view: Option<ViewHandle<ExternalEditorView>>,
    remote_file_auto_open_text_max_mib_editor: ViewHandle<EditorView>,
    remote_file_text_cache_max_mib_editor: ViewHandle<EditorView>,
    remote_file_large_preview_max_kib_editor: ViewHandle<EditorView>,
}

impl CodeSettingsPageView {
    pub fn new(ctx: &mut ViewContext<CodeSettingsPageView>) -> Self {
        // 订阅 ProjectContextModel:project rules 变动时重渲染,
        // 让任何依赖 rule 集合的子组件保持最新。
        ctx.subscribe_to_model(&ProjectContextModel::handle(ctx), |_me, _, event, ctx| {
            if matches!(event, ProjectContextModelEvent::KnownRulesChanged(_)) {
                ctx.notify();
            }
        });

        let code_settings = CodeSettings::as_ref(ctx);
        let remote_file_auto_open_text_max_mib = *code_settings.remote_file_auto_open_text_max_mib;
        let remote_file_text_cache_max_mib = *code_settings.remote_file_text_cache_max_mib;
        let remote_file_large_preview_max_kib = *code_settings.remote_file_large_preview_max_kib;
        let remote_file_auto_open_text_max_mib_editor = Self::number_editor(
            remote_file_auto_open_text_max_mib,
            |view, event, ctx| view.handle_remote_file_auto_open_text_max_mib_editor(event, ctx),
            ctx,
        );
        let remote_file_text_cache_max_mib_editor = Self::number_editor(
            remote_file_text_cache_max_mib,
            |view, event, ctx| view.handle_remote_file_text_cache_max_mib_editor(event, ctx),
            ctx,
        );
        let remote_file_large_preview_max_kib_editor = Self::number_editor(
            remote_file_large_preview_max_kib,
            |view, event, ctx| view.handle_remote_file_large_preview_max_kib_editor(event, ctx),
            ctx,
        );
        let (page, external_editor_view) = Self::build_page(ctx);

        Self {
            page,
            #[cfg(feature = "local_fs")]
            external_editor_view,
            remote_file_auto_open_text_max_mib_editor,
            remote_file_text_cache_max_mib_editor,
            remote_file_large_preview_max_kib_editor,
        }
    }

    fn number_editor(
        initial_value: u64,
        handler: fn(&mut Self, &EditorEvent, &mut ViewContext<Self>),
        ctx: &mut ViewContext<Self>,
    ) -> ViewHandle<EditorView> {
        let editor = ctx.add_typed_action_view(move |ctx| {
            let mut editor = EditorView::single_line(
                SingleLineEditorOptions {
                    clear_selections_on_blur: true,
                    text: TextOptions::ui_font_size(Appearance::as_ref(ctx)),
                    ..Default::default()
                },
                ctx,
            );
            editor.set_buffer_text(&initial_value.to_string(), ctx);
            editor
        });
        ctx.subscribe_to_view(&editor, move |view, _, event, ctx| {
            handler(view, event, ctx);
        });
        editor
    }

    /// 构造页面 widgets。Code 现在是单页(无子页面、无 category 标题),
    /// 直接铺平展示「编辑器与代码评审」开关。
    #[cfg(feature = "local_fs")]
    fn build_page(
        ctx: &mut ViewContext<Self>,
    ) -> (PageType<Self>, Option<ViewHandle<ExternalEditorView>>) {
        let (widgets, external_editor_view) = if FeatureFlag::ZapNewSettingsModes.is_enabled() {
            let editor_view = ctx.add_typed_action_view(ExternalEditorView::new);
            let widgets: Vec<Box<dyn SettingsWidget<View = Self>>> = vec![
                Box::new(ExternalEditorCodeWidget),
                Box::new(AutoOpenCodeReviewPaneCodeWidget::default()),
                Box::new(CodeReviewPanelToggleWidget::default()),
                Box::new(CodeReviewDiffStatsToggleWidget::default()),
                Box::new(ProjectExplorerToggleWidget::default()),
                Box::new(ShowHiddenFilesToggleWidget::default()),
                Box::new(ShowLineNumbersToggleWidget::default()),
                Box::new(AutoSaveToggleWidget::default()),
                Box::new(GlobalSearchToggleWidget::default()),
                Box::new(RemoteFileAutoOpenTextMaxMiBWidget),
                Box::new(RemoteFileTextCacheMaxMiBWidget),
                Box::new(RemoteFileLargePreviewMaxKiBWidget),
            ];
            (widgets, Some(editor_view))
        } else {
            // legacy 视图:旧设置模式下 Code 页不渲染任何内容(原 CodePageWidget
            // 仅渲染一个 LSP 时代的 header,无实际意义,直接返回空页面)。
            (vec![], None)
        };
        (
            PageType::new_uncategorized(widgets, None),
            external_editor_view,
        )
    }

    /// wasm 构建下没有 ExternalEditorView,只渲染 4 个非外部编辑器开关。
    #[cfg(not(feature = "local_fs"))]
    fn build_page(
        _ctx: &mut ViewContext<Self>,
    ) -> (PageType<Self>, Option<ViewHandle<ExternalEditorView>>) {
        let widgets: Vec<Box<dyn SettingsWidget<View = Self>>> =
            if FeatureFlag::ZapNewSettingsModes.is_enabled() {
                vec![
                    Box::new(AutoOpenCodeReviewPaneCodeWidget::default()),
                    Box::new(CodeReviewPanelToggleWidget::default()),
                    Box::new(CodeReviewDiffStatsToggleWidget::default()),
                    Box::new(ProjectExplorerToggleWidget::default()),
                    Box::new(ShowHiddenFilesToggleWidget::default()),
                    Box::new(ShowLineNumbersToggleWidget::default()),
                    Box::new(AutoSaveToggleWidget::default()),
                    Box::new(GlobalSearchToggleWidget::default()),
                    Box::new(RemoteFileAutoOpenTextMaxMiBWidget),
                    Box::new(RemoteFileTextCacheMaxMiBWidget),
                    Box::new(RemoteFileLargePreviewMaxKiBWidget),
                ]
            } else {
                vec![]
            };
        (PageType::new_uncategorized(widgets, None), None)
    }
}

impl Entity for CodeSettingsPageView {
    type Event = CodeSettingsPageEvent;
}

impl View for CodeSettingsPageView {
    fn ui_name() -> &'static str {
        "CodePage"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        self.page.render(self, app)
    }
}

#[derive(Debug, Clone)]
pub enum CodeSettingsPageEvent {
    OpenProjectRules { rule_paths: Vec<PathBuf> },
    FocusModal,
}

#[derive(Debug, Clone)]
pub enum CodeSettingsPageAction {
    OpenProjectRules { rule_paths: Vec<PathBuf> },
    ToggleCodeReviewPanel,
    ToggleShowCodeReviewDiffStats,
    ToggleAutoOpenCodeReviewPane,
    ToggleProjectExplorer,
    ToggleShowHiddenFiles,
    ToggleShowLineNumbers,
    ToggleAutoSave,
    ToggleGlobalSearch,
    SetRemoteFileAutoOpenTextMaxMiB,
    SetRemoteFileTextCacheMaxMiB,
    SetRemoteFileLargePreviewMaxKiB,
}

impl TypedActionView for CodeSettingsPageView {
    type Action = CodeSettingsPageAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            CodeSettingsPageAction::OpenProjectRules { rule_paths } => {
                ctx.emit(CodeSettingsPageEvent::OpenProjectRules {
                    rule_paths: rule_paths.clone(),
                });
            }
            CodeSettingsPageAction::ToggleCodeReviewPanel => {
                TabSettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings.show_code_review_button.toggle_and_save_value(ctx));
                });
                ctx.notify();
            }
            CodeSettingsPageAction::ToggleShowCodeReviewDiffStats => {
                TabSettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings
                        .show_code_review_diff_stats
                        .toggle_and_save_value(ctx));
                });
                ctx.notify();
            }
            CodeSettingsPageAction::ToggleProjectExplorer => {
                CodeSettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings.show_project_explorer.toggle_and_save_value(ctx));
                });
                ctx.notify();
            }
            CodeSettingsPageAction::ToggleShowHiddenFiles => {
                CodeSettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings.show_hidden_files.toggle_and_save_value(ctx));
                });
                ctx.notify();
            }
            CodeSettingsPageAction::ToggleShowLineNumbers => {
                CodeSettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings.show_line_numbers.toggle_and_save_value(ctx));
                });
                ctx.notify();
            }
            CodeSettingsPageAction::ToggleAutoSave => {
                CodeSettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings.auto_save.toggle_and_save_value(ctx));
                });
                ctx.notify();
            }
            CodeSettingsPageAction::ToggleGlobalSearch => {
                CodeSettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings.show_global_search.toggle_and_save_value(ctx));
                });
                ctx.notify();
            }
            CodeSettingsPageAction::SetRemoteFileAutoOpenTextMaxMiB => {
                self.set_remote_file_auto_open_text_max_mib(ctx);
            }
            CodeSettingsPageAction::SetRemoteFileTextCacheMaxMiB => {
                self.set_remote_file_text_cache_max_mib(ctx);
            }
            CodeSettingsPageAction::SetRemoteFileLargePreviewMaxKiB => {
                self.set_remote_file_large_preview_max_kib(ctx);
            }
            CodeSettingsPageAction::ToggleAutoOpenCodeReviewPane => {
                GeneralSettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings
                        .auto_open_code_review_pane_on_first_agent_change
                        .toggle_and_save_value(ctx));
                });
                send_telemetry_from_ctx!(
                    TelemetryEvent::FeaturesPageAction {
                        action: "ToggleAutoOpenCodeReviewPane".to_string(),
                        value: format!(
                            "{}",
                            *GeneralSettings::as_ref(ctx)
                                .auto_open_code_review_pane_on_first_agent_change
                        )
                    },
                    ctx
                );
                ctx.notify();
            }
        }
    }
}

impl CodeSettingsPageView {
    fn handle_remote_file_auto_open_text_max_mib_editor(
        &mut self,
        event: &EditorEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        match event {
            EditorEvent::Blurred | EditorEvent::Enter => {
                self.set_remote_file_auto_open_text_max_mib(ctx);
                if matches!(event, EditorEvent::Enter) {
                    ctx.emit(CodeSettingsPageEvent::FocusModal);
                }
            }
            EditorEvent::Escape => ctx.emit(CodeSettingsPageEvent::FocusModal),
            _ => {}
        }
    }

    fn handle_remote_file_text_cache_max_mib_editor(
        &mut self,
        event: &EditorEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        match event {
            EditorEvent::Blurred | EditorEvent::Enter => {
                self.set_remote_file_text_cache_max_mib(ctx);
                if matches!(event, EditorEvent::Enter) {
                    ctx.emit(CodeSettingsPageEvent::FocusModal);
                }
            }
            EditorEvent::Escape => ctx.emit(CodeSettingsPageEvent::FocusModal),
            _ => {}
        }
    }

    fn handle_remote_file_large_preview_max_kib_editor(
        &mut self,
        event: &EditorEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        match event {
            EditorEvent::Blurred | EditorEvent::Enter => {
                self.set_remote_file_large_preview_max_kib(ctx);
                if matches!(event, EditorEvent::Enter) {
                    ctx.emit(CodeSettingsPageEvent::FocusModal);
                }
            }
            EditorEvent::Escape => ctx.emit(CodeSettingsPageEvent::FocusModal),
            _ => {}
        }
    }

    fn set_remote_file_auto_open_text_max_mib(&mut self, ctx: &mut ViewContext<Self>) {
        self.set_remote_file_number_setting(
            self.remote_file_auto_open_text_max_mib_editor.clone(),
            REMOTE_FILE_AUTO_OPEN_TEXT_MIN_MIB,
            REMOTE_FILE_AUTO_OPEN_TEXT_MAX_MIB,
            |settings, value, ctx| {
                report_if_error!(settings
                    .remote_file_auto_open_text_max_mib
                    .set_value(value, ctx));
            },
            |settings| *settings.remote_file_auto_open_text_max_mib,
            ctx,
        );
    }

    fn set_remote_file_text_cache_max_mib(&mut self, ctx: &mut ViewContext<Self>) {
        self.set_remote_file_number_setting(
            self.remote_file_text_cache_max_mib_editor.clone(),
            REMOTE_FILE_TEXT_CACHE_MIN_MIB,
            REMOTE_FILE_TEXT_CACHE_MAX_MIB,
            |settings, value, ctx| {
                report_if_error!(settings
                    .remote_file_text_cache_max_mib
                    .set_value(value, ctx));
            },
            |settings| *settings.remote_file_text_cache_max_mib,
            ctx,
        );
    }

    fn set_remote_file_large_preview_max_kib(&mut self, ctx: &mut ViewContext<Self>) {
        self.set_remote_file_number_setting(
            self.remote_file_large_preview_max_kib_editor.clone(),
            REMOTE_FILE_LARGE_PREVIEW_MIN_KIB,
            REMOTE_FILE_LARGE_PREVIEW_MAX_KIB,
            |settings, value, ctx| {
                report_if_error!(settings
                    .remote_file_large_preview_max_kib
                    .set_value(value, ctx));
            },
            |settings| *settings.remote_file_large_preview_max_kib,
            ctx,
        );
    }

    fn set_remote_file_number_setting(
        &mut self,
        editor: ViewHandle<EditorView>,
        min: u64,
        max: u64,
        save: fn(&mut CodeSettings, u64, &mut ModelContext<CodeSettings>),
        current: fn(&CodeSettings) -> u64,
        ctx: &mut ViewContext<Self>,
    ) {
        let raw_value = editor.as_ref(ctx).buffer_text(ctx);
        let cleaned: String = raw_value
            .chars()
            .filter(|c| !c.is_whitespace() && *c != ',')
            .collect();
        let parsed = cleaned.parse::<u64>().ok();
        CodeSettings::handle(ctx).update(ctx, |settings, ctx| {
            if let Some(value) = parsed {
                save(settings, value.clamp(min, max), ctx);
            }
        });
        let value = current(CodeSettings::as_ref(ctx)).clamp(min, max);
        editor.update(ctx, |editor, ctx| {
            editor.system_reset_buffer_text(&value.to_string(), ctx);
        });
        ctx.notify();
    }
}

pub fn init_actions_from_parent_view<T: Action + Clone>(
    _app: &mut AppContext,
    _context: &ContextPredicate,
    _builder: fn(SettingsAction) -> T,
) {
}

#[cfg(feature = "local_fs")]
struct ExternalEditorCodeWidget;

#[cfg(feature = "local_fs")]
impl SettingsWidget for ExternalEditorCodeWidget {
    type View = CodeSettingsPageView;

    fn search_terms(&self) -> &str {
        "code editor open files markdown AI conversations layout pane tab"
    }

    fn render(
        &self,
        view: &Self::View,
        _appearance: &Appearance,
        _app: &AppContext,
    ) -> Box<dyn Element> {
        if let Some(editor_view) = &view.external_editor_view {
            ChildView::new(editor_view).finish()
        } else {
            Empty::new().finish()
        }
    }
}

#[derive(Default)]
struct AutoOpenCodeReviewPaneCodeWidget {
    switch_state: SwitchStateHandle,
}

impl SettingsWidget for AutoOpenCodeReviewPaneCodeWidget {
    type View = CodeSettingsPageView;

    fn search_terms(&self) -> &str {
        "oz auto open code review pane panel agent mode change first time accepted diff view conversation"
    }

    fn render(
        &self,
        _view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let general_settings = GeneralSettings::as_ref(app);
        render_body_item::<CodeSettingsPageAction>(
            crate::t!("settings-code-auto-open-review-panel"),
            None,
            LocalOnlyIconState::Hidden,
            ToggleState::Enabled,
            appearance,
            appearance
                .ui_builder()
                .switch(self.switch_state.clone())
                .check(*general_settings.auto_open_code_review_pane_on_first_agent_change)
                .build()
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(CodeSettingsPageAction::ToggleAutoOpenCodeReviewPane);
                })
                .finish(),
            Some(crate::t!("settings-code-auto-open-review-panel-desc")),
        )
    }
}

impl SettingsPageMeta for CodeSettingsPageView {
    fn section() -> SettingsSection {
        SettingsSection::Code
    }

    fn update_filter(&mut self, query: &str, ctx: &mut ViewContext<Self>) -> MatchData {
        self.page.update_filter(query, ctx)
    }

    fn should_render(&self, _ctx: &AppContext) -> bool {
        FeatureFlag::ZapNewSettingsModes.is_enabled()
    }

    fn on_page_selected(&mut self, _: bool, _ctx: &mut ViewContext<Self>) {}

    fn scroll_to_widget(&mut self, widget_id: &'static str) {
        self.page.scroll_to_widget(widget_id)
    }

    fn clear_highlighted_widget(&mut self) {
        self.page.clear_highlighted_widget();
    }
}

impl From<ViewHandle<CodeSettingsPageView>> for SettingsPageViewHandle {
    fn from(view_handle: ViewHandle<CodeSettingsPageView>) -> Self {
        SettingsPageViewHandle::Code(view_handle)
    }
}

#[derive(Default)]
struct CodeReviewPanelToggleWidget {
    switch_state: SwitchStateHandle,
}

impl SettingsWidget for CodeReviewPanelToggleWidget {
    type View = CodeSettingsPageView;

    fn search_terms(&self) -> &str {
        "code review panel right side diff git"
    }

    fn render(
        &self,
        _view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let tab_settings = TabSettings::as_ref(app);

        render_body_item::<CodeSettingsPageAction>(
            crate::t!("settings-code-show-code-review-button"),
            None,
            LocalOnlyIconState::Hidden,
            ToggleState::Enabled,
            appearance,
            appearance
                .ui_builder()
                .switch(self.switch_state.clone())
                .check(*tab_settings.show_code_review_button)
                .build()
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(CodeSettingsPageAction::ToggleCodeReviewPanel);
                })
                .finish(),
            Some(crate::t!("settings-code-show-code-review-button-desc")),
        )
    }
}

#[derive(Default)]
struct CodeReviewDiffStatsToggleWidget {
    switch_state: SwitchStateHandle,
}

impl SettingsWidget for CodeReviewDiffStatsToggleWidget {
    type View = CodeSettingsPageView;

    fn search_terms(&self) -> &str {
        "code review diff stats lines added removed counts"
    }

    fn render(
        &self,
        _view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let tab_settings = TabSettings::as_ref(app);

        render_body_item::<CodeSettingsPageAction>(
            crate::t!("settings-code-show-diff-stats"),
            None,
            LocalOnlyIconState::Hidden,
            ToggleState::Enabled,
            appearance,
            appearance
                .ui_builder()
                .switch(self.switch_state.clone())
                .check(*tab_settings.show_code_review_diff_stats)
                .build()
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(
                        CodeSettingsPageAction::ToggleShowCodeReviewDiffStats,
                    );
                })
                .finish(),
            Some(crate::t!("settings-code-show-diff-stats-desc")),
        )
    }
}

#[derive(Default)]
struct ProjectExplorerToggleWidget {
    switch_state: SwitchStateHandle,
}

impl SettingsWidget for ProjectExplorerToggleWidget {
    type View = CodeSettingsPageView;

    fn search_terms(&self) -> &str {
        "project explorer file tree left panel tools"
    }

    fn render(
        &self,
        _view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let code_settings = CodeSettings::as_ref(app);

        render_body_item::<CodeSettingsPageAction>(
            crate::t!("settings-code-project-explorer"),
            None,
            LocalOnlyIconState::Hidden,
            ToggleState::Enabled,
            appearance,
            appearance
                .ui_builder()
                .switch(self.switch_state.clone())
                .check(*code_settings.show_project_explorer)
                .build()
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(CodeSettingsPageAction::ToggleProjectExplorer);
                })
                .finish(),
            Some(crate::t!("settings-code-project-explorer-desc")),
        )
    }
}

#[derive(Default)]
struct ShowHiddenFilesToggleWidget {
    switch_state: SwitchStateHandle,
}

impl SettingsWidget for ShowHiddenFilesToggleWidget {
    type View = CodeSettingsPageView;

    fn search_terms(&self) -> &str {
        "hidden files dotfiles project explorer file tree"
    }

    fn render(
        &self,
        _view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let code_settings = CodeSettings::as_ref(app);

        render_body_item::<CodeSettingsPageAction>(
            crate::t!("settings-code-show-hidden-files"),
            None,
            LocalOnlyIconState::Hidden,
            ToggleState::Enabled,
            appearance,
            appearance
                .ui_builder()
                .switch(self.switch_state.clone())
                .check(*code_settings.show_hidden_files)
                .build()
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(CodeSettingsPageAction::ToggleShowHiddenFiles);
                })
                .finish(),
            Some(crate::t!("settings-code-show-hidden-files-desc")),
        )
    }
}

#[derive(Default)]
struct ShowLineNumbersToggleWidget {
    switch_state: SwitchStateHandle,
}

impl SettingsWidget for ShowLineNumbersToggleWidget {
    type View = CodeSettingsPageView;

    fn search_terms(&self) -> &str {
        "line numbers gutter code editor"
    }

    fn render(
        &self,
        _view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let code_settings = CodeSettings::as_ref(app);

        render_body_item::<CodeSettingsPageAction>(
            crate::t!("settings-code-show-line-numbers"),
            None,
            LocalOnlyIconState::Hidden,
            ToggleState::Enabled,
            appearance,
            appearance
                .ui_builder()
                .switch(self.switch_state.clone())
                .check(*code_settings.show_line_numbers)
                .build()
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(CodeSettingsPageAction::ToggleShowLineNumbers);
                })
                .finish(),
            Some(crate::t!("settings-code-show-line-numbers-desc")),
        )
    }
}

#[derive(Default)]
struct AutoSaveToggleWidget {
    switch_state: SwitchStateHandle,
}

impl SettingsWidget for AutoSaveToggleWidget {
    type View = CodeSettingsPageView;

    fn search_terms(&self) -> &str {
        "auto save autosave save after typing editor files"
    }

    fn render(
        &self,
        _view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let code_settings = CodeSettings::as_ref(app);

        render_body_item::<CodeSettingsPageAction>(
            crate::t!("settings-code-auto-save"),
            None,
            LocalOnlyIconState::Hidden,
            ToggleState::Enabled,
            appearance,
            appearance
                .ui_builder()
                .switch(self.switch_state.clone())
                .check(*code_settings.auto_save)
                .build()
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(CodeSettingsPageAction::ToggleAutoSave);
                })
                .finish(),
            Some(crate::t!("settings-code-auto-save-desc")),
        )
    }
}

struct RemoteFileAutoOpenTextMaxMiBWidget;

impl SettingsWidget for RemoteFileAutoOpenTextMaxMiBWidget {
    type View = CodeSettingsPageView;

    fn search_terms(&self) -> &str {
        "remote file sftp ssh auto open text size limit mib"
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        _app: &AppContext,
    ) -> Box<dyn Element> {
        render_remote_file_number_item(
            "远程文本自动打开上限 (MiB)".to_string(),
            "小于或等于该大小的远程文本文件会直接在 Zora 编辑器中打开。范围 1-64 MiB。".to_string(),
            view.remote_file_auto_open_text_max_mib_editor.clone(),
            CodeSettingsPageAction::SetRemoteFileAutoOpenTextMaxMiB,
            appearance,
        )
    }
}

struct RemoteFileTextCacheMaxMiBWidget;

impl SettingsWidget for RemoteFileTextCacheMaxMiBWidget {
    type View = CodeSettingsPageView;

    fn search_terms(&self) -> &str {
        "remote file sftp ssh text cache memory budget mib"
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        _app: &AppContext,
    ) -> Box<dyn Element> {
        render_remote_file_number_item(
            "远程文本缓存总量 (MiB)".to_string(),
            "远程文本文件的内存 LRU 缓存预算，0 表示禁用缓存。范围 0-512 MiB。".to_string(),
            view.remote_file_text_cache_max_mib_editor.clone(),
            CodeSettingsPageAction::SetRemoteFileTextCacheMaxMiB,
            appearance,
        )
    }
}

struct RemoteFileLargePreviewMaxKiBWidget;

impl SettingsWidget for RemoteFileLargePreviewMaxKiBWidget {
    type View = CodeSettingsPageView;

    fn search_terms(&self) -> &str {
        "remote file sftp ssh large preview sniff kib"
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        _app: &AppContext,
    ) -> Box<dyn Element> {
        render_remote_file_number_item(
            "远程大文件预览读取大小 (KiB)".to_string(),
            "超过自动打开上限时用于生成预览和识别文本类型的远程文件前缀大小。范围 256-8192 KiB。"
                .to_string(),
            view.remote_file_large_preview_max_kib_editor.clone(),
            CodeSettingsPageAction::SetRemoteFileLargePreviewMaxKiB,
            appearance,
        )
    }
}

fn render_remote_file_number_item(
    label: String,
    description: String,
    editor: ViewHandle<EditorView>,
    action: CodeSettingsPageAction,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let dismiss_editor = editor.clone();
    let input = Dismiss::new(
        appearance
            .ui_builder()
            .text_input(editor)
            .with_style(UiComponentStyles {
                width: Some(REMOTE_FILE_NUMBER_INPUT_WIDTH),
                padding: Some(Coords {
                    top: appearance.ui_font_size() / 2.,
                    bottom: appearance.ui_font_size() / 2.,
                    left: appearance.ui_font_size() * 5. / 6.,
                    right: appearance.ui_font_size() * 5. / 6.,
                }),
                background: Some(appearance.theme().surface_2().into()),
                ..Default::default()
            })
            .build()
            .finish(),
    )
    .on_dismiss(move |ctx, app| {
        if !dismiss_editor
            .as_ref(app)
            .buffer_text(app)
            .trim()
            .is_empty()
        {
            ctx.dispatch_typed_action(action.clone());
        }
    })
    .finish();

    render_body_item::<CodeSettingsPageAction>(
        label,
        None,
        LocalOnlyIconState::Hidden,
        ToggleState::Enabled,
        appearance,
        input,
        Some(description),
    )
}

#[derive(Default)]
struct GlobalSearchToggleWidget {
    switch_state: SwitchStateHandle,
}

impl SettingsWidget for GlobalSearchToggleWidget {
    type View = CodeSettingsPageView;

    fn search_terms(&self) -> &str {
        "global search file search left panel tools"
    }

    fn render(
        &self,
        _view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let code_settings = CodeSettings::as_ref(app);

        render_body_item::<CodeSettingsPageAction>(
            crate::t!("settings-code-global-search"),
            None,
            LocalOnlyIconState::Hidden,
            ToggleState::Enabled,
            appearance,
            appearance
                .ui_builder()
                .switch(self.switch_state.clone())
                .check(*code_settings.show_global_search)
                .build()
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(CodeSettingsPageAction::ToggleGlobalSearch);
                })
                .finish(),
            Some(crate::t!("settings-code-global-search-desc")),
        )
    }
}
