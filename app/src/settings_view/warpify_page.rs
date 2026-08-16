use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Display;

use markdown_parser::{FormattedText, FormattedTextFragment, FormattedTextLine};
use regex::Regex;
use settings::{Setting, ToggleableSetting};
use strum::IntoEnumIterator;
use warp_core::features::FeatureFlag;
use warpui::elements::{Dismiss, FormattedTextElement, HighlightedHyperlink, Text};
use warpui::keymap::ContextPredicate;
use warpui::{
    elements::{Container, Flex, Hoverable, MouseStateHandle, ParentElement},
    platform::Cursor,
    presenter::ChildView,
    ui_components::{
        components::{Coords, UiComponent, UiComponentStyles},
        switch::SwitchStateHandle,
    },
    Action, AppContext, Element, Entity, ModelContext, ModelHandle, SingletonEntity,
    TypedActionView, View, ViewContext, ViewHandle,
};

use crate::code::global_buffer_model::GlobalBufferModel;
use crate::editor::{EditorView, Event as EditorEvent, SingleLineEditorOptions, TextOptions};
use crate::terminal::warpify::settings::{
    EnableSshWarpification, SshExtensionInstallMode, SshExtensionInstallModeSetting,
    UseSshTmuxWrapper, WarpifySettingsChangedEvent,
};
use crate::ui_components::blended_colors;
use crate::{
    appearance::Appearance,
    report_if_error, send_telemetry_from_ctx,
    server::telemetry::TelemetryEvent,
    settings::{
        CodeSettings, REMOTE_FILE_AUTO_OPEN_TEXT_MAX_MIB, REMOTE_FILE_AUTO_OPEN_TEXT_MIN_MIB,
        REMOTE_FILE_LARGE_PREVIEW_MAX_KIB, REMOTE_FILE_LARGE_PREVIEW_MIN_KIB,
        REMOTE_FILE_TEXT_CACHE_MAX_MIB, REMOTE_FILE_TEXT_CACHE_MIN_MIB,
    },
    terminal::warpify::settings::WarpifySettings,
    view_components::{SubmittableTextInput, SubmittableTextInputEvent},
};

use super::settings_page::{
    render_body_item, render_dropdown_item, render_page_title, AdditionalInfo, Category,
    LocalOnlyIconState, MatchData, PageType, SettingsPageEvent, SettingsWidget, ToggleState,
    HEADER_PADDING,
};
use super::SettingsSection;
use super::{
    flags,
    settings_page::{
        add_setting, render_alternating_color_list, SettingsPageMeta, SettingsPageViewHandle,
    },
    SettingsAction, ToggleSettingActionPair,
};
use crate::view_components::dropdown::{Dropdown, DropdownItem};

pub fn init_actions_from_parent_view<T: Action + Clone>(
    app: &mut AppContext,
    context: &ContextPredicate,
    builder: fn(SettingsAction) -> T,
) {
    // Add all of the toggle settings from the Warpify Page that you want to show up on the Command Palette here.
    let mut toggle_binding_pairs = vec![];

    if FeatureFlag::SSHTmuxWrapper.is_enabled() {
        toggle_binding_pairs.push(ToggleSettingActionPair::new(
            &crate::t!("settings-warpify-ssh-tmux-toggle-binding-label"),
            builder(SettingsAction::WarpifyPageToggle(
                WarpifyPageAction::ToggleTmuxWarpification,
            )),
            context,
            flags::SSH_TMUX_WRAPPER_CONTEXT_FLAG,
        ));
    }

    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(toggle_binding_pairs, app);
}

const ITEM_VERTICAL_SPACING: f32 = 24.;
/// There's a built-in 10px margin below the text input.
const BUILT_IN_TEXT_INPUT_MARGIN: f32 = 10.;
const SPACE_AFTER_TEXT_INPUT: f32 = ITEM_VERTICAL_SPACING - BUILT_IN_TEXT_INPUT_MARGIN;
const REMOTE_FILE_NUMBER_INPUT_WIDTH: f32 = 96.;

/// This page lets users configure when they get asked to warpify a session. Some shell commands
/// are recognized by default. Users can add new shell commands, or prevent the default ones from
/// asking. Users can also enable the SSH wrapper, and add hosts to a denylist.
/// This page is essentially the View for the SubshellSettings model, as well as the SshSettings
/// related to warpification.
pub struct WarpifyPageView {
    page: PageType<Self>,
    /// This needs to mirror the length of SubshellSettings::added_remove_button_states.
    remove_added_command_button_states: Vec<MouseStateHandle>,
    add_added_commands_editor: ViewHandle<SubmittableTextInput>,
    /// This needs to mirror the length of SubshellSettings::denylisted_remove_button_states.
    remove_denylisted_command_button_states: Vec<MouseStateHandle>,
    add_denylisted_commands_editor: ViewHandle<SubmittableTextInput>,

    remove_denylisted_ssh_button_states: Vec<MouseStateHandle>,
    add_denylisted_ssh_editor: ViewHandle<SubmittableTextInput>,

    ssh_extension_install_mode_dropdown: ViewHandle<Dropdown<WarpifyPageAction>>,
    remote_file_auto_open_text_max_mib_editor: ViewHandle<EditorView>,
    remote_file_text_cache_max_mib_editor: ViewHandle<EditorView>,
    remote_file_large_preview_max_kib_editor: ViewHandle<EditorView>,
}

impl WarpifyPageView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let warpify_settings_handle = WarpifySettings::handle(ctx);

        ctx.observe(&warpify_settings_handle, Self::update_button_states);
        ctx.subscribe_to_model(&warpify_settings_handle, move |me, model, event, ctx| {
            me.update_button_states(model, ctx);
            if matches!(
                event,
                WarpifySettingsChangedEvent::SshExtensionInstallModeSetting { .. }
            ) {
                me.update_dropdown(ctx);
            }
            ctx.notify();
        });

        // Added commands can be specified by regex, while denied commands are strictly exact
        // match.
        let add_added_commands_editor = ctx.add_typed_action_view(|ctx| {
            let mut input =
                SubmittableTextInput::new(ctx).validate_on_edit(|regex| Regex::new(regex).is_ok());
            input.set_placeholder_text(crate::t!("settings-warpify-command-placeholder"), ctx);
            input
        });

        ctx.subscribe_to_view(
            &add_added_commands_editor,
            Self::handle_added_command_editor_event,
        );

        let add_denylisted_commands_editor = ctx.add_typed_action_view(|ctx| {
            let mut input = SubmittableTextInput::new(ctx);
            input.set_placeholder_text(crate::t!("settings-warpify-command-placeholder"), ctx);
            input
        });

        ctx.subscribe_to_view(
            &add_denylisted_commands_editor,
            Self::handle_denylisted_command_editor_event,
        );

        let add_denylisted_ssh_editor = ctx.add_typed_action_view(|ctx| {
            let mut input = SubmittableTextInput::new(ctx);
            input.set_placeholder_text(crate::t!("settings-warpify-host-placeholder"), ctx);
            input
        });

        ctx.subscribe_to_view(
            &add_denylisted_ssh_editor,
            Self::handle_denylisted_ssh_editor_event,
        );

        let ssh_extension_install_mode_dropdown =
            Self::create_ssh_extension_install_mode_dropdown(ctx);
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

        let mut instance = Self {
            page: Self::build_page(ctx),
            remove_added_command_button_states: Default::default(),
            add_added_commands_editor,
            remove_denylisted_command_button_states: Default::default(),
            add_denylisted_commands_editor,
            remove_denylisted_ssh_button_states: Default::default(),
            add_denylisted_ssh_editor,
            ssh_extension_install_mode_dropdown,
            remote_file_auto_open_text_max_mib_editor,
            remote_file_text_cache_max_mib_editor,
            remote_file_large_preview_max_kib_editor,
        };

        instance.update_button_states(warpify_settings_handle, ctx);
        instance
    }

    fn build_page(ctx: &mut ViewContext<Self>) -> PageType<Self> {
        let mut categories = vec![
            Category::new("", vec![Box::new(TitleWidget::default())]),
            Category::new(
                Box::leak(crate::t!("settings-warpify-section-subshells").into_boxed_str()),
                vec![Box::new(SubshellsWidget::default())],
            )
            .with_subtitle(Box::leak(
                crate::t!("settings-warpify-section-subshells-subtitle").into_boxed_str(),
            )),
        ];

        let warpify_settings = WarpifySettings::as_ref(ctx);
        if FeatureFlag::SSHTmuxWrapper.is_enabled()
            && warpify_settings
                .enable_ssh_warpification
                .is_supported_on_current_platform()
        {
            categories.push(
                Category::new(
                    Box::leak(crate::t!("settings-warpify-section-ssh").into_boxed_str()),
                    vec![Box::new(SSHWidget::default())],
                )
                .with_subtitle(Box::leak(
                    crate::t!("settings-warpify-section-ssh-subtitle").into_boxed_str(),
                )),
            );
        }
        categories.push(
            Category::new(
                "SSH / SFTP 远程文件",
                vec![Box::new(RemoteFileSettingsWidget::default())],
            )
            .with_subtitle("配置远程文本打开、预览和内存缓存。"),
        );
        PageType::new_categorized(categories, None)
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

    /// This method ensures each command in the SubshellSettings has a matching button state for
    /// its delete button in the View.
    fn update_button_states(
        &mut self,
        warpify_settings_handle: ModelHandle<WarpifySettings>,
        ctx: &mut ViewContext<Self>,
    ) {
        let warpify_settings = warpify_settings_handle.as_ref(ctx);
        self.remove_denylisted_command_button_states = warpify_settings
            .subshell_command_denylist
            .iter()
            .map(|_| Default::default())
            .collect();
        self.remove_added_command_button_states = warpify_settings
            .added_subshell_commands
            .iter()
            .map(|_| Default::default())
            .collect();
        self.remove_denylisted_ssh_button_states = warpify_settings
            .ssh_hosts_denylist
            .iter()
            .map(|_| Default::default())
            .collect();
        ctx.notify();
    }

    /// Syncs the install-mode dropdown selection with the current
    /// `WarpifySettings::ssh_extension_install_mode` value (e.g. after it
    /// was changed from the SSH remote server choice view).
    fn update_dropdown(&mut self, ctx: &mut ViewContext<Self>) {
        let current_mode = *WarpifySettings::as_ref(ctx)
            .ssh_extension_install_mode
            .value();
        self.ssh_extension_install_mode_dropdown
            .update(ctx, |dropdown, ctx| {
                dropdown.set_selected_by_action(
                    WarpifyPageAction::SetSshExtensionInstallMode(current_mode),
                    ctx,
                );
            });
    }

    fn handle_added_command_editor_event(
        &mut self,
        _handle: ViewHandle<SubmittableTextInput>,
        event: &SubmittableTextInputEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        match event {
            SubmittableTextInputEvent::Submit(new_command) => {
                WarpifySettings::handle(ctx).update(ctx, |warpify_settings, ctx| {
                    warpify_settings.add_subshell_command(new_command, ctx);
                });

                send_telemetry_from_ctx!(TelemetryEvent::AddAddedSubshellCommand, ctx);
            }
            SubmittableTextInputEvent::Escape => ctx.emit(SettingsPageEvent::FocusModal),
        }
    }

    fn handle_denylisted_command_editor_event(
        &mut self,
        _handle: ViewHandle<SubmittableTextInput>,
        event: &SubmittableTextInputEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        match event {
            SubmittableTextInputEvent::Submit(new_command) => {
                WarpifySettings::handle(ctx).update(ctx, |warpify_settings, ctx| {
                    warpify_settings.denylist_subshell_command(new_command, ctx);
                });

                send_telemetry_from_ctx!(TelemetryEvent::AddDenylistedSubshellCommand, ctx);
            }
            SubmittableTextInputEvent::Escape => ctx.emit(SettingsPageEvent::FocusModal),
        }
    }

    fn handle_denylisted_ssh_editor_event(
        &mut self,
        _handle: ViewHandle<SubmittableTextInput>,
        event: &SubmittableTextInputEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        match event {
            SubmittableTextInputEvent::Submit(new_command) => {
                WarpifySettings::handle(ctx).update(ctx, |warpify_settings, ctx| {
                    warpify_settings.denylist_ssh_host(new_command, ctx);
                });

                send_telemetry_from_ctx!(TelemetryEvent::AddDenylistedSshTmuxWrapperHost, ctx);
            }
            SubmittableTextInputEvent::Escape => ctx.emit(SettingsPageEvent::FocusModal),
        }
    }

    fn remove_denylisted_command(&self, index: usize, ctx: &mut ViewContext<Self>) {
        send_telemetry_from_ctx!(TelemetryEvent::RemoveDenylistedSubshellCommand, ctx);
        WarpifySettings::handle(ctx).update(ctx, |warpify, ctx| {
            warpify.remove_denylisted_subshell_command(index, ctx)
        });
    }

    fn remove_added_command(&self, index: usize, ctx: &mut ViewContext<Self>) {
        send_telemetry_from_ctx!(TelemetryEvent::RemoveAddedSubshellCommand, ctx);
        WarpifySettings::handle(ctx).update(ctx, |warpify, ctx| {
            warpify.remove_added_subshell_command(index, ctx)
        });
    }

    fn remove_denylisted_ssh_host(&self, index: usize, ctx: &mut ViewContext<Self>) {
        send_telemetry_from_ctx!(TelemetryEvent::RemoveDenylistedSshTmuxWrapperHost, ctx);
        WarpifySettings::handle(ctx).update(ctx, |warpify, ctx| {
            warpify.remove_denylisted_ssh_host(index, ctx)
        });
    }
}

impl Entity for WarpifyPageView {
    type Event = SettingsPageEvent;
}

fn build_sub_sub_title(title: String, appearance: &Appearance) -> Container {
    appearance
        .ui_builder()
        .span(title)
        .with_style(UiComponentStyles {
            font_size: Some(appearance.ui_font_body()),
            ..Default::default()
        })
        .build()
}

const SSH_EXTENSION_DROPDOWN_WIDTH: f32 = 250.;

impl WarpifyPageView {
    fn create_ssh_extension_install_mode_dropdown(
        ctx: &mut ViewContext<Self>,
    ) -> ViewHandle<Dropdown<WarpifyPageAction>> {
        let items: Vec<DropdownItem<WarpifyPageAction>> = SshExtensionInstallMode::iter()
            .map(|mode| {
                DropdownItem::new(
                    mode.display_name(),
                    WarpifyPageAction::SetSshExtensionInstallMode(mode),
                )
            })
            .collect();

        let current_mode = *WarpifySettings::as_ref(ctx)
            .ssh_extension_install_mode
            .value();
        let enable_ssh_warpification = *WarpifySettings::as_ref(ctx)
            .enable_ssh_warpification
            .value();

        ctx.add_typed_action_view(move |ctx| {
            let mut dropdown = Dropdown::new(ctx);
            dropdown.set_top_bar_max_width(SSH_EXTENSION_DROPDOWN_WIDTH);
            dropdown.set_menu_width(SSH_EXTENSION_DROPDOWN_WIDTH, ctx);
            dropdown.add_items(items, ctx);
            dropdown.set_selected_by_action(
                WarpifyPageAction::SetSshExtensionInstallMode(current_mode),
                ctx,
            );
            if !enable_ssh_warpification {
                dropdown.set_disabled(ctx);
            }
            dropdown
        })
    }

    /// Renders a title, a list of items that can be removed, and an input field to add new items.
    fn build_input_list<
        ListItem: Display,
        SettingsPageAction: Action + Clone,
        F: Fn(usize) -> SettingsPageAction,
        T: View,
    >(
        &self,
        title: String,
        patterns: &[ListItem],
        mouse_states: &[MouseStateHandle],
        create_action: F,
        handle: &ViewHandle<T>,
        appearance: &Appearance,
    ) -> Container {
        let mut column = Flex::column();
        let mut title = build_sub_sub_title(title, appearance);

        if !patterns.is_empty() {
            title = title.with_padding_bottom(BUILT_IN_TEXT_INPUT_MARGIN);
        }

        column.add_child(title.finish());

        render_alternating_color_list(
            &mut column,
            patterns,
            mouse_states,
            create_action,
            appearance,
        );

        Container::new(
            column
                .with_child(
                    Container::new(ChildView::new(handle).finish())
                        .with_margin_bottom(SPACE_AFTER_TEXT_INPUT)
                        .finish(),
                )
                .finish(),
        )
    }
}

impl View for WarpifyPageView {
    fn ui_name() -> &'static str {
        "WarpifyPageView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        self.page.render(self, app)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum WarpifyPageAction {
    RemoveAddedCommand(usize),
    RemoveDenylistedCommand(usize),
    RemoveDenylistedSshHost(usize),
    /// If disabled, auto-Warpification and the SSH Warpification prompt will be disabled.
    ToggleTmuxWarpification,
    ToggleSshWarpification,
    /// Set the SSH extension installation mode (always ask / always install / always skip).
    SetSshExtensionInstallMode(SshExtensionInstallMode),
    OpenUrl(String),
    SetRemoteFileAutoOpenTextMaxMiB,
    SetRemoteFileTextCacheMaxMiB,
    SetRemoteFileLargePreviewMaxKiB,
    ClearRemoteFileTextCache,
}

impl TypedActionView for WarpifyPageView {
    type Action = WarpifyPageAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        use WarpifyPageAction::*;
        match action {
            RemoveDenylistedCommand(index) => self.remove_denylisted_command(*index, ctx),
            RemoveAddedCommand(index) => self.remove_added_command(*index, ctx),
            ToggleSshWarpification => {
                WarpifySettings::handle(ctx).update(ctx, |ssh_settings, ctx| {
                    report_if_error!(ssh_settings
                        .enable_ssh_warpification
                        .toggle_and_save_value(ctx));
                    send_telemetry_from_ctx!(
                        TelemetryEvent::ToggleSshWarpification {
                            enabled: *ssh_settings.enable_ssh_warpification.value(),
                        },
                        ctx
                    );
                });
                let enabled = *WarpifySettings::as_ref(ctx)
                    .enable_ssh_warpification
                    .value();
                self.ssh_extension_install_mode_dropdown
                    .update(ctx, |dropdown, ctx| {
                        if enabled {
                            dropdown.set_enabled(ctx);
                        } else {
                            dropdown.set_disabled(ctx);
                        }
                    });
            }
            ToggleTmuxWarpification => {
                WarpifySettings::handle(ctx).update(ctx, |ssh_settings, ctx| {
                    report_if_error!(ssh_settings.use_ssh_tmux_wrapper.toggle_and_save_value(ctx));
                    send_telemetry_from_ctx!(
                        TelemetryEvent::ToggleSshTmuxWrapper {
                            enabled: *ssh_settings.use_ssh_tmux_wrapper.value(),
                        },
                        ctx
                    );
                });
            }
            SetSshExtensionInstallMode(mode) => {
                WarpifySettings::handle(ctx).update(ctx, |warpify_settings, ctx| {
                    report_if_error!(warpify_settings
                        .ssh_extension_install_mode
                        .set_value(*mode, ctx));
                    send_telemetry_from_ctx!(
                        TelemetryEvent::SetSshExtensionInstallMode {
                            mode: mode.telemetry_name(),
                        },
                        ctx
                    );
                });
            }
            WarpifyPageAction::RemoveDenylistedSshHost(index) => {
                self.remove_denylisted_ssh_host(*index, ctx);
            }
            OpenUrl(url) => {
                ctx.open_url(url.as_str());
            }
            SetRemoteFileAutoOpenTextMaxMiB => {
                self.set_remote_file_auto_open_text_max_mib(ctx);
            }
            SetRemoteFileTextCacheMaxMiB => {
                self.set_remote_file_text_cache_max_mib(ctx);
            }
            SetRemoteFileLargePreviewMaxKiB => {
                self.set_remote_file_large_preview_max_kib(ctx);
            }
            ClearRemoteFileTextCache => {
                GlobalBufferModel::handle(ctx).update(ctx, |model, _ctx| {
                    model.clear_sftp_text_cache();
                });
            }
        }
    }
}

impl WarpifyPageView {
    fn handle_remote_file_auto_open_text_max_mib_editor(
        &mut self,
        event: &EditorEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        match event {
            EditorEvent::Blurred | EditorEvent::Enter => {
                self.set_remote_file_auto_open_text_max_mib(ctx);
            }
            EditorEvent::Escape => ctx.focus_self(),
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
            }
            EditorEvent::Escape => ctx.focus_self(),
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
            }
            EditorEvent::Escape => ctx.focus_self(),
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

impl SettingsPageMeta for WarpifyPageView {
    fn section() -> SettingsSection {
        SettingsSection::Warpify
    }

    fn should_render(&self, _ctx: &AppContext) -> bool {
        true
    }

    fn update_filter(&mut self, query: &str, ctx: &mut ViewContext<Self>) -> MatchData {
        self.page.update_filter(query, ctx)
    }

    fn scroll_to_widget(&mut self, widget_id: &'static str) {
        self.page.scroll_to_widget(widget_id)
    }

    fn clear_highlighted_widget(&mut self) {
        self.page.clear_highlighted_widget();
    }
}

impl From<ViewHandle<WarpifyPageView>> for SettingsPageViewHandle {
    fn from(view_handle: ViewHandle<WarpifyPageView>) -> Self {
        SettingsPageViewHandle::Warpify(view_handle)
    }
}

#[derive(Default)]
struct TitleWidget {
    learn_more_highlight_index: HighlightedHyperlink,
}

impl TitleWidget {
    fn render_top_of_page(&self, appearance: &Appearance, _app: &AppContext) -> Box<dyn Element> {
        let warpify_description = vec![
            FormattedTextFragment::plain_text(crate::t!("settings-warpify-description-prefix")),
            FormattedTextFragment::hyperlink(crate::t!("settings-warpify-learn-more"), ""),
        ];

        let warpify_description = FormattedTextElement::new(
            FormattedText::new([FormattedTextLine::Line(warpify_description)]),
            appearance.ui_font_body(),
            appearance.ui_font_family(),
            appearance.ui_font_family(),
            blended_colors::text_sub(appearance.theme(), appearance.theme().surface_1()),
            self.learn_more_highlight_index.clone(),
        )
        .with_line_height_ratio(appearance.ui_line_height_ratio())
        .with_heading_to_font_size_multipliers(appearance.heading_font_size_multipliers().clone())
        .with_hyperlink_font_color(appearance.theme().accent().into_solid())
        .register_default_click_handlers(|url, _, ctx| {
            ctx.open_url(&url.url);
        })
        .finish();

        Flex::column()
            .with_child(render_page_title(
                &crate::t!("settings-warpify-page-title"),
                appearance,
            ))
            .with_child(warpify_description)
            .finish()
    }
}

impl SettingsWidget for TitleWidget {
    type View = WarpifyPageView;

    fn search_terms(&self) -> &str {
        "ssh subshell warpify session"
    }

    fn render(
        &self,
        _view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        Container::new(self.render_top_of_page(appearance, app))
            .with_margin_bottom(ITEM_VERTICAL_SPACING)
            .finish()
    }
}

#[derive(Default)]
struct SubshellsWidget {}

impl SubshellsWidget {
    fn render_subshells_section(
        &self,
        view: &WarpifyPageView,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let mut column = Flex::column();

        let warpify_settings = WarpifySettings::as_ref(app);

        column.add_child(
            view.build_input_list(
                crate::t!("settings-warpify-added-commands"),
                &warpify_settings.added_subshell_commands,
                &view.remove_added_command_button_states,
                WarpifyPageAction::RemoveAddedCommand,
                &view.add_added_commands_editor,
                appearance,
            )
            .finish(),
        );

        column.add_child(
            view.build_input_list(
                crate::t!("settings-warpify-denylisted-commands"),
                &warpify_settings.subshell_command_denylist,
                &view.remove_denylisted_command_button_states,
                WarpifyPageAction::RemoveDenylistedCommand,
                &view.add_denylisted_commands_editor,
                appearance,
            )
            .with_margin_bottom(-BUILT_IN_TEXT_INPUT_MARGIN)
            .finish(),
        );

        column.finish()
    }
}

impl SettingsWidget for SubshellsWidget {
    type View = WarpifyPageView;

    fn search_terms(&self) -> &str {
        "warpify subshell"
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        Container::new(self.render_subshells_section(view, appearance, app))
            .with_margin_bottom(ITEM_VERTICAL_SPACING)
            .finish()
    }
}

#[derive(Default)]
struct RemoteFileSettingsWidget {
    clear_cache_button: MouseStateHandle,
}

impl SettingsWidget for RemoteFileSettingsWidget {
    type View = WarpifyPageView;

    fn search_terms(&self) -> &str {
        "ssh sftp remote file text cache preview open"
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        _app: &AppContext,
    ) -> Box<dyn Element> {
        let mut column = Flex::column();
        column.add_child(render_remote_file_number_item(
            "远程文本自动打开上限 (MiB)".to_string(),
            "小于或等于该大小的远程文本文件会直接在 Zora 编辑器中打开。范围 1-64 MiB。".to_string(),
            view.remote_file_auto_open_text_max_mib_editor.clone(),
            WarpifyPageAction::SetRemoteFileAutoOpenTextMaxMiB,
            appearance,
        ));
        column.add_child(render_remote_file_number_item(
            "远程文本缓存总量 (MiB)".to_string(),
            "远程文本文件的内存 LRU 缓存预算，0 表示禁用缓存。范围 0-512 MiB。".to_string(),
            view.remote_file_text_cache_max_mib_editor.clone(),
            WarpifyPageAction::SetRemoteFileTextCacheMaxMiB,
            appearance,
        ));
        column.add_child(render_remote_file_number_item(
            "远程大文件预览读取大小 (KiB)".to_string(),
            "超过自动打开上限时用于生成预览和识别文本类型的远程文件前缀大小。范围 256-8192 KiB。"
                .to_string(),
            view.remote_file_large_preview_max_kib_editor.clone(),
            WarpifyPageAction::SetRemoteFileLargePreviewMaxKiB,
            appearance,
        ));
        column.add_child(render_body_item::<WarpifyPageAction>(
            "远程文本缓存".to_string(),
            None,
            LocalOnlyIconState::Hidden,
            ToggleState::Enabled,
            appearance,
            render_clear_cache_button(self.clear_cache_button.clone(), appearance),
            Some("清空当前进程内的远程文本内存缓存；不会删除远程文件。".to_string()),
        ));
        Container::new(column.finish())
            .with_margin_bottom(ITEM_VERTICAL_SPACING)
            .finish()
    }
}

fn render_remote_file_number_item(
    label: String,
    description: String,
    editor: ViewHandle<EditorView>,
    action: WarpifyPageAction,
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

    render_body_item::<WarpifyPageAction>(
        label,
        None,
        LocalOnlyIconState::Hidden,
        ToggleState::Enabled,
        appearance,
        input,
        Some(description),
    )
}

fn render_clear_cache_button(
    mouse_state: MouseStateHandle,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let text_color = appearance.theme().active_ui_text_color();
    let background = appearance.theme().surface_2();
    let hover_background = appearance.theme().surface_3();
    let pressed_background = appearance.theme().background();
    let outline = appearance.theme().outline();
    let pressed_outline = appearance.theme().surface_3();
    let font = appearance.ui_font_family();
    let font_size = appearance.ui_font_size();
    Hoverable::new(mouse_state, move |state| {
        let (button_background, button_outline) = if state.is_clicked() {
            (pressed_background, pressed_outline)
        } else if state.is_hovered() {
            (hover_background, outline)
        } else {
            (background, outline)
        };

        Container::new(
            Text::new_inline("清空缓存", font, font_size)
                .with_color(text_color.into())
                .finish(),
        )
        .with_padding_left(font_size)
        .with_padding_right(font_size)
        .with_padding_top(font_size / 2.0)
        .with_padding_bottom(font_size / 2.0)
        .with_background(button_background)
        .with_border(warpui::elements::Border::all(1.0).with_border_fill(button_outline))
        .with_corner_radius(warpui::elements::CornerRadius::with_all(
            warpui::elements::Radius::Pixels(4.0),
        ))
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(|ctx, _, _| {
        ctx.dispatch_typed_action(WarpifyPageAction::ClearRemoteFileTextCache);
    })
    .finish()
}

#[derive(Default)]
struct SSHWidget {
    tmux_warpification_switch_state: SwitchStateHandle,
    enable_ssh_warpification_switch_state: SwitchStateHandle,
    additional_info_mouse_state: MouseStateHandle,
    local_only_icon_tooltip_states: RefCell<HashMap<String, MouseStateHandle>>,
}

impl SettingsWidget for SSHWidget {
    type View = WarpifyPageView;

    fn search_terms(&self) -> &str {
        "warpify ssh"
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let mut column = Flex::column();
        let ui_builder = appearance.ui_builder();
        let description_text_color = appearance
            .theme()
            .sub_text_color(appearance.theme().surface_2());

        let enable_ssh_warpification = *WarpifySettings::as_ref(app)
            .enable_ssh_warpification
            .value();

        let should_prompt_ssh_tmux_wrapper =
            *WarpifySettings::as_ref(app).use_ssh_tmux_wrapper.value();

        add_setting(
            &mut column,
            &WarpifySettings::as_ref(app).enable_ssh_warpification,
            move || {
                render_body_item::<WarpifyPageAction>(
                    crate::t!("settings-warpify-enable-ssh"),
                    None,
                    LocalOnlyIconState::for_setting(
                        EnableSshWarpification::storage_key(),
                        EnableSshWarpification::sync_to_cloud(),
                        &mut self.local_only_icon_tooltip_states.borrow_mut(),
                        app,
                    ),
                    ToggleState::Enabled,
                    appearance,
                    ui_builder
                        .switch(self.enable_ssh_warpification_switch_state.clone())
                        .check(enable_ssh_warpification)
                        .build()
                        .on_click(move |ctx, _, _| {
                            ctx.dispatch_typed_action(WarpifyPageAction::ToggleSshWarpification);
                        })
                        .finish(),
                    None,
                )
            },
        );

        if FeatureFlag::SshRemoteServer.is_enabled() {
            let label_color_override = if !enable_ssh_warpification {
                Some(appearance.theme().disabled_ui_text_color())
            } else {
                None
            };
            add_setting(
                &mut column,
                &WarpifySettings::as_ref(app).ssh_extension_install_mode,
                move || {
                    let install_ssh_label = crate::t!("settings-warpify-install-ssh-extension");
                    let install_ssh_desc =
                        crate::t!("settings-warpify-install-ssh-extension-description");
                    Container::new(render_dropdown_item(
                        appearance,
                        &install_ssh_label,
                        Some(&install_ssh_desc),
                        None,
                        LocalOnlyIconState::for_setting(
                            SshExtensionInstallModeSetting::storage_key(),
                            SshExtensionInstallModeSetting::sync_to_cloud(),
                            &mut self.local_only_icon_tooltip_states.borrow_mut(),
                            app,
                        ),
                        label_color_override,
                        &view.ssh_extension_install_mode_dropdown,
                    ))
                    .with_padding_bottom(HEADER_PADDING)
                    .finish()
                },
            );
        }

        add_setting(
            &mut column,
            &WarpifySettings::as_ref(app).use_ssh_tmux_wrapper,
            move || {
                let mut column = Flex::column();

                column.add_child(render_body_item::<WarpifyPageAction>(
                    crate::t!("settings-warpify-use-tmux"),
                    Some(AdditionalInfo {
                        mouse_state: self.additional_info_mouse_state.clone(),
                        on_click_action: Some(WarpifyPageAction::OpenUrl("".into())),
                        secondary_text: None,
                        tooltip_override_text: None,
                    }),
                    LocalOnlyIconState::for_setting(
                        UseSshTmuxWrapper::storage_key(),
                        UseSshTmuxWrapper::sync_to_cloud(),
                        &mut self.local_only_icon_tooltip_states.borrow_mut(),
                        app,
                    ),
                    enable_ssh_warpification.into(),
                    appearance,
                    ui_builder
                        .switch(self.tmux_warpification_switch_state.clone())
                        .check(should_prompt_ssh_tmux_wrapper)
                        .with_disabled(!enable_ssh_warpification)
                        .build()
                        .on_click(move |ctx, _, _| {
                            if !enable_ssh_warpification {
                                return;
                            }

                            ctx.dispatch_typed_action(WarpifyPageAction::ToggleTmuxWarpification);
                        })
                        .finish(),
                    None,
                ));

                column.add_child(
                    ui_builder
                        .paragraph(crate::t!("settings-warpify-tmux-description"))
                        .with_style(UiComponentStyles {
                            font_color: Some(description_text_color.into_solid()),
                            margin: Some(
                                Coords::default()
                                    .top(styles::DESCRIPTION_NEGATIVE_MARGIN_OFFSET)
                                    .bottom(styles::DESCRIPTION_LINE_MARGIN_BOTTOM),
                            ),
                            ..Default::default()
                        })
                        .build()
                        .finish(),
                );

                if enable_ssh_warpification && should_prompt_ssh_tmux_wrapper {
                    let warpify_settings = WarpifySettings::as_ref(app);
                    column.add_child(
                        view.build_input_list(
                            crate::t!("settings-warpify-denylisted-hosts"),
                            &warpify_settings.ssh_hosts_denylist,
                            &view.remove_denylisted_ssh_button_states,
                            WarpifyPageAction::RemoveDenylistedSshHost,
                            &view.add_denylisted_ssh_editor,
                            appearance,
                        )
                        .finish(),
                    );
                } else {
                    // Add margin to hint the user should scroll to see more.
                    column.add_child(
                        Container::new(Flex::column().finish())
                            .with_margin_bottom(styles::MINIMUM_SCROLL_OFFSET_AFTER_SSH)
                            .finish(),
                    );
                }

                column.finish()
            },
        );

        column.finish()
    }
}

mod styles {
    // Apply a negative margin to the description text so it appears closer to the main
    // settings option text.
    pub const DESCRIPTION_NEGATIVE_MARGIN_OFFSET: f32 = -8.;

    /// The space after a description.
    pub const DESCRIPTION_LINE_MARGIN_BOTTOM: f32 = 18.;

    /// Because we hide the SSH settings if the SSH wrapper is disabled, we need to add a margin
    /// to the bottom to make it clear that toggling this item will reveal more settings,
    /// even at smaller window sizes. We picked an offset that cuts off the first item
    /// to imply the user should scroll to see more.
    pub const MINIMUM_SCROLL_OFFSET_AFTER_SSH: f32 = 40.;
}
