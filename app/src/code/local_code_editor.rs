/// This module contains a model that can be used for loading and saving text files
/// and displaying them in a code editor.
/// It also handles applying an optional diff to the file content that will be applied
/// when the file is loaded.
//
// LSP 全栈下线后,本文件不再承载任何 LSP / hover / goto-definition /
// find-references / 诊断装饰相关逻辑;只保留文件 load/save、diff 接受/拒绝、
// 选区上下文 tooltip、版本冲突横幅、TabConfig footer 等本地能力。
use std::{
    ops::Range,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

use pathfinder_geometry::vector::Vector2F;
use warp_core::{features::FeatureFlag, ui::appearance::Appearance};
use warp_editor::{content::buffer::InitialBufferState, render::model::LineCount};
use warp_util::{
    content_version::ContentVersion,
    file::{FileId, FileLoadError, FileSaveError},
    path::to_relative_path,
};
use warpui::platform::SaveFilePickerConfiguration;
use warpui::{
    elements::{
        Border, ChildAnchor, ChildView, Clipped, ConstrainedBox, Container, CornerRadius,
        CrossAxisAlignment, DropShadow, Flex, Hoverable, MainAxisAlignment, MainAxisSize,
        MouseStateHandle, OffsetPositioning, ParentAnchor, ParentElement, ParentOffsetBounds,
        Radius, Rect, Shrinkable, Stack, Text,
    },
    keymap::{macros::*, FixedBinding},
    text::point::Point,
    ui_components::{
        button::ButtonVariant,
        components::{Coords, UiComponent, UiComponentStyles},
    },
    AppContext, Element, Entity, SingletonEntity, TypedActionView, View, ViewContext, ViewHandle,
    WindowId,
};

use crate::sftp_manager::sftp_ops::normalize_remote_path;
use crate::{
    code::{
        buffer_location::SftpPath,
        footer::{CodeFooterView, CodeFooterViewEvent},
        global_buffer_model::{BufferState, GlobalBufferModel},
        SaveOutcome,
    },
    debounce::debounce,
    settings::{AISettings, CodeSettings},
    terminal::TerminalView,
    util::sync::Condition,
};
use crate::{
    code::{editor::EditorReviewComment, global_buffer_model::GlobalBufferModelEvent},
    code_review::comments::CommentId,
    editor::{EditorView, Event as EditorEvent, SingleLineEditorOptions, TextColors, TextOptions},
};
use ai::diff_validation::DiffType;
use pathfinder_color::ColorU;
use vim::vim::{MotionType, VimMode};
use warp_core::ui::icons::Icon;

use crate::workspace::WorkspaceAction;

const DROP_SHADOW_COLOR: ColorU = ColorU {
    r: 0,
    g: 0,
    b: 0,
    a: 48,
};

const AUTO_SAVE_DEBOUNCE_PERIOD: Duration = Duration::from_millis(1000);

use super::diff_viewer::DiffViewer;
use super::editor::{
    scroll::{ScrollPosition, ScrollTrigger},
    view::{CodeEditorEvent, CodeEditorView},
};
use super::ImmediateSaveError;

type SaveCallback =
    Box<dyn FnOnce(SaveOutcome, &mut ViewContext<LocalCodeEditorView>) + Send + Sync + 'static>;

pub fn init(app: &mut AppContext) {
    app.register_fixed_bindings([FixedBinding::new(
        "cmdorctrl-l",
        LocalCodeEditorAction::InsertSelectedTextToInput,
        id!("LocalCodeEditorView") & !id!("IMEOpen"),
    )]);
}

fn make_sftp_save_as_path_editor(
    ctx: &mut ViewContext<LocalCodeEditorView>,
) -> ViewHandle<EditorView> {
    ctx.add_typed_action_view(|ctx| {
        let appearance = Appearance::as_ref(ctx);
        let theme = appearance.theme();
        let options = SingleLineEditorOptions {
            text: TextOptions {
                font_size_override: Some(appearance.ui_font_size()),
                font_family_override: Some(appearance.monospace_font_family()),
                text_colors_override: Some(TextColors {
                    default_color: theme.active_ui_text_color(),
                    disabled_color: theme.disabled_ui_text_color(),
                    hint_color: theme.disabled_ui_text_color(),
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut editor = EditorView::single_line(options, ctx);
        editor.set_placeholder_text("远程保存路径", ctx);
        editor
    })
}

pub enum LocalCodeEditorEvent {
    FileLoaded,
    FailedToLoad {
        error: Rc<FileLoadError>,
    },
    FileSaved {
        auto_saved: bool,
    },
    FailedToSave {
        error: Rc<FileSaveError>,
    },
    /// The SFTP backing file changed remotely before this editor saved.
    SftpSaveConflict,
    /// The user requested remote Save As for an SFTP conflict.
    SftpSaveAsRequested,
    DiffAccepted,
    DiffRejected,
    /// Emitted when a user presses Escape in Vim Normal mode inside the embedded editor.
    VimMinimizeRequested,
    /// Emitted when a user edits the file.
    UserEdited,
    /// Emitted when the diff status changes (e.g., line counts update).
    DiffStatusUpdated,
    SelectionAddedAsContext {
        relative_file_path: String,
        /// 1-indexed line range of the selection: `[start, end]` both inclusive.
        line_range: Range<LineCount>,
        /// Literal text content of the selection.
        selected_text: String,
    },
    DiscardUnsavedChanges {
        path: PathBuf,
    },
    /// Emitted when a comment is saved. This propagates the comment content
    /// changes to the CodeReviewView, which will update the comment model.
    CommentSaved {
        comment: EditorReviewComment,
    },
    RequestOpenComment(CommentId),
    DeleteComment {
        id: CommentId,
    },
    /// Emitted when the viewport is updated after layout
    ViewportUpdated,
    /// Emitted when the render state layout has been updated.
    LayoutInvalidated,
    /// TabConfig footer 上点击「/update-tab-config」后递到上层处理。
    RunTabConfigSkill {
        path: PathBuf,
    },
    DelayedRenderingFlushed,
}

/// Metadata about a file that is opened in the code view.
#[derive(Debug, Clone)]
enum LoadedFileMetadata {
    /// Normal file with both FileId and path (for files that are actually opened)
    LocalFile { id: FileId, path: PathBuf },
    /// 远端 buffer:文件位于 SSH 主机上,通过 buffer-sync 协议同步,
    /// 本地没有对应路径。
    #[cfg_attr(not(feature = "local_tty"), allow(dead_code))]
    RemoteFile {
        id: FileId,
        remote_path: crate::code::buffer_location::RemotePath,
    },
    /// SFTP buffer:文件位于 SSH 管理器连接上,通过 SftpBackend 读写。
    SftpFile {
        id: FileId,
        sftp_path: crate::code::buffer_location::SftpPath,
    },
}

pub use super::diff_viewer::DisplayMode;

type TerminalTargetFn = dyn Fn(WindowId, &AppContext) -> Option<ViewHandle<TerminalView>>;

struct SelectionAsContextTooltip {
    mouse_state: MouseStateHandle,
    terminal_target_fn: Box<TerminalTargetFn>,
}

#[derive(Debug, Clone)]
pub enum LocalCodeEditorAction {
    InsertSelectedTextToInput,
    SaveFile,
    DiscardUnsavedChanges,
    ReloadSftpFromRemote,
    ForceSaveSftp,
    SaveSftpAs,
    ConfirmSaveSftpAs,
    CancelSftpConflict,
}

#[derive(Default)]
struct ConflictResolutionBannerMouseStates {
    discard_mouse_state: MouseStateHandle,
    overwrite_mouse_state: MouseStateHandle,
    reload_sftp_mouse_state: MouseStateHandle,
    force_save_sftp_mouse_state: MouseStateHandle,
    save_sftp_as_mouse_state: MouseStateHandle,
    confirm_sftp_save_as_mouse_state: MouseStateHandle,
    cancel_sftp_conflict_mouse_state: MouseStateHandle,
}

pub struct LocalCodeEditorView {
    pub(super) editor: ViewHandle<CodeEditorView>,
    metadata: Option<LoadedFileMetadata>,
    enable_diff_nav_by_default: bool,
    is_new_file: bool,
    diff_type: Option<DiffType>,
    selection_as_context_tooltip: Option<SelectionAsContextTooltip>,
    /// A marker for when the backing file has first been loaded. This is used to prevent applying
    /// a diff before it can be properly calculated.
    file_loaded: Condition,
    /// Whether content was changed from its base.
    was_edited: bool,
    /// Content version of the base file state.
    base_content_version: Option<ContentVersion>,
    conflict_banner_mouse_states: ConflictResolutionBannerMouseStates,
    /// Default directory to use for save dialogs when creating new files
    default_directory: Option<PathBuf>,
    /// Footer for displaying TabConfig actions. Only created for tab config TOML files.
    footer: Option<ViewHandle<CodeFooterView>>,
    /// Pending scroll position to apply after the file is loaded. This is used when
    /// `set_pending_scroll` is called before the file content has finished loading
    /// (e.g., in the GlobalBuffer path where content loads asynchronously).
    pending_scroll_on_load: Option<ScrollPosition>,
    auto_save_debounce_tx: async_channel::Sender<()>,
    auto_save_in_flight: bool,
    sftp_save_conflict_pending: bool,
    sftp_save_as_open: bool,
    pending_sftp_save_as_path: Option<SftpPath>,
    sftp_save_as_path_editor: ViewHandle<EditorView>,
}

impl LocalCodeEditorView {
    pub fn new(
        editor: ViewHandle<CodeEditorView>,
        diff_type: Option<DiffType>,
        enable_diff_nav_by_default: bool,
        display_mode: Option<DisplayMode>,
        ctx: &mut ViewContext<Self>,
    ) -> Self {
        ctx.subscribe_to_view(&editor, |me, _, event, ctx| match event {
            CodeEditorEvent::UnifiedDiffComputed(_) => {
                ctx.emit(LocalCodeEditorEvent::DiffAccepted);
            }
            CodeEditorEvent::ContentChanged { origin, .. } => {
                me.update_diff_hunk_gutter_buttons(ctx);

                if origin.from_user() {
                    me.was_edited = true;
                    ctx.emit(LocalCodeEditorEvent::UserEdited);

                    if me.diff_type.is_none() && *CodeSettings::as_ref(ctx).auto_save {
                        let _ = me.auto_save_debounce_tx.try_send(());
                    }
                }
            }
            CodeEditorEvent::VimEscapeInNormalMode => {
                ctx.emit(LocalCodeEditorEvent::VimMinimizeRequested);
            }
            CodeEditorEvent::EscapePressed => {}
            CodeEditorEvent::DiffUpdated => {
                ctx.emit(LocalCodeEditorEvent::DiffStatusUpdated);
            }
            CodeEditorEvent::SelectionEnd => {
                ctx.notify();
            }
            CodeEditorEvent::MouseHovered { .. } => {
                // LSP 下线后,鼠标 hover 不再触发 hover/goto-definition;保留 event 订阅但不做处理。
            }
            CodeEditorEvent::CommentSaved { comment } => {
                ctx.emit(LocalCodeEditorEvent::CommentSaved {
                    comment: comment.clone(),
                });
            }
            CodeEditorEvent::DeleteComment { id } => {
                ctx.emit(LocalCodeEditorEvent::DeleteComment { id: *id });
            }
            CodeEditorEvent::RequestOpenComment(uuid) => {
                ctx.emit(LocalCodeEditorEvent::RequestOpenComment(*uuid));
            }
            CodeEditorEvent::ViewportUpdated => {
                ctx.emit(LocalCodeEditorEvent::ViewportUpdated);
            }
            CodeEditorEvent::LayoutInvalidated => {
                ctx.emit(LocalCodeEditorEvent::LayoutInvalidated);
            }
            CodeEditorEvent::DelayedRenderingFlushed => {
                ctx.emit(LocalCodeEditorEvent::DelayedRenderingFlushed);
            }
            CodeEditorEvent::Focused
            | CodeEditorEvent::SelectionChanged
            | CodeEditorEvent::SelectionStart
            | CodeEditorEvent::CopiedEmptyText
            | CodeEditorEvent::DiffHunkContextAdded { .. }
            | CodeEditorEvent::DiffReverted
            | CodeEditorEvent::HiddenSectionExpanded => {}
            #[cfg(windows)]
            CodeEditorEvent::WindowsCtrlC { .. } => {}
        });

        let is_new_file = matches!(diff_type, Some(DiffType::Create { .. }));
        let (auto_save_debounce_tx, auto_save_debounce_rx) = async_channel::unbounded();
        ctx.spawn_stream_local(
            debounce(AUTO_SAVE_DEBOUNCE_PERIOD, auto_save_debounce_rx),
            |me, (), ctx| me.auto_save_after_delay(ctx),
            |_, _| {},
        );

        let sftp_save_as_path_editor = make_sftp_save_as_path_editor(ctx);
        let save_as_editor_handle = sftp_save_as_path_editor.clone();
        ctx.subscribe_to_view(&save_as_editor_handle, |me, _, event, ctx| match event {
            EditorEvent::Enter => me.confirm_sftp_save_as(ctx),
            EditorEvent::Escape => {
                me.sftp_save_as_open = false;
                me.pending_sftp_save_as_path = None;
                ctx.notify();
            }
            _ => {}
        });

        let model = Self {
            editor,
            diff_type,
            is_new_file,
            metadata: None,
            enable_diff_nav_by_default,
            file_loaded: Condition::new(),
            selection_as_context_tooltip: None,
            was_edited: false,
            base_content_version: None,
            conflict_banner_mouse_states: Default::default(),
            default_directory: None,
            footer: None,
            pending_scroll_on_load: None,
            auto_save_debounce_tx,
            auto_save_in_flight: false,
            sftp_save_conflict_pending: false,
            sftp_save_as_open: false,
            pending_sftp_save_as_path: None,
            sftp_save_as_path_editor,
        };

        if let Some(display_mode) = display_mode {
            model.set_display_mode(display_mode, ctx);
        }
        model
    }

    fn current_sftp_path(&self) -> Option<&SftpPath> {
        match self.metadata.as_ref()? {
            LoadedFileMetadata::SftpFile { sftp_path, .. } => Some(sftp_path),
            LoadedFileMetadata::LocalFile { .. } | LoadedFileMetadata::RemoteFile { .. } => None,
        }
    }

    fn suggested_sftp_save_as_path(path: &Path) -> PathBuf {
        let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
        let file_name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "untitled".to_string());
        let suggested_name = match (path.file_stem(), path.extension()) {
            (Some(stem), Some(ext)) => {
                format!("{}.copy.{}", stem.to_string_lossy(), ext.to_string_lossy())
            }
            (Some(stem), None) => format!("{}.copy", stem.to_string_lossy()),
            (None, _) => format!("{file_name}.copy"),
        };

        normalize_remote_path(
            &parent
                .map(|parent| parent.join(suggested_name.clone()))
                .unwrap_or_else(|| PathBuf::from("/").join(suggested_name)),
        )
    }

    fn resolve_sftp_save_as_path(current_path: &Path, input: &str) -> Option<PathBuf> {
        let trimmed = input.trim().replace('\\', "/");
        if trimmed.is_empty() {
            return None;
        }

        let resolved = if trimmed.starts_with('/') {
            trimmed
        } else {
            let current = normalize_remote_path(&current_path.to_path_buf());
            let current = current.to_string_lossy();
            let parent = current
                .rsplit_once('/')
                .map(|(parent, _)| if parent.is_empty() { "/" } else { parent })
                .unwrap_or("/");
            if parent == "/" {
                format!("/{trimmed}")
            } else {
                format!("{parent}/{trimmed}")
            }
        };
        if resolved
            .split('/')
            .any(|component| component == "." || component == "..")
        {
            return None;
        }
        Some(normalize_remote_path(&PathBuf::from(resolved)))
    }

    fn begin_sftp_save_as(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(sftp_path) = self.current_sftp_path() else {
            return;
        };
        let suggested = Self::suggested_sftp_save_as_path(&sftp_path.path);
        self.sftp_save_as_path_editor.update(ctx, |editor, ctx| {
            editor.set_buffer_text(&suggested.display().to_string(), ctx);
        });
        self.sftp_save_as_open = true;
        self.pending_sftp_save_as_path = None;
        ctx.focus(&self.sftp_save_as_path_editor);
        ctx.notify();
    }

    fn confirm_sftp_save_as(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(file_id) = self.file_id() else {
            return;
        };
        let Some(current_sftp_path) = self.current_sftp_path().cloned() else {
            return;
        };
        let input = self.sftp_save_as_path_editor.as_ref(ctx).buffer_text(ctx);
        let Some(path) = Self::resolve_sftp_save_as_path(&current_sftp_path.path, &input) else {
            ctx.emit(LocalCodeEditorEvent::FailedToSave {
                error: Rc::new(FileSaveError::RemoteError("远程另存为路径无效".to_string())),
            });
            return;
        };
        let new_sftp_path = SftpPath {
            node_id: current_sftp_path.node_id.clone(),
            path,
            display_identity: current_sftp_path.display_identity.clone(),
        };
        self.pending_sftp_save_as_path = Some(new_sftp_path.clone());
        GlobalBufferModel::handle(ctx).update(ctx, |model, ctx| {
            model.save_sftp_buffer_as(file_id, new_sftp_path, ctx);
        });
    }

    fn perform_save(&mut self, file_id: FileId, ctx: &mut ViewContext<Self>) {
        self.base_content_version = Some(self.editor.as_ref(ctx).version(ctx));

        // 远端 SSH 文件:走 buffer-sync 的 `SaveBuffer` 协议落盘。不能用下面的本地
        // `FileModel` 路径 —— Remote buffer 没有本地路径,会得到 `NoFilePath`。
        // 用户的编辑已通过 `BufferEdit` 实时同步到 daemon,这里只触发 daemon 落盘。
        #[cfg(feature = "local_tty")]
        {
            let is_remote = GlobalBufferModel::handle(ctx)
                .as_ref(ctx)
                .is_remote(file_id);
            if is_remote {
                GlobalBufferModel::handle(ctx)
                    .update(ctx, |model, ctx| model.save_remote_buffer(file_id, ctx));
                return;
            }
        }

        let result = match self.diff() {
            Some(DiffType::Update {
                rename: Some(new_path),
                ..
            }) => self.editor.update(ctx, |editor, ctx| {
                let content = editor.text(ctx);
                let buffer_version = editor.version(ctx);

                GlobalBufferModel::handle(ctx).update(ctx, move |model, ctx| {
                    model.rename_and_save(
                        file_id,
                        new_path.clone(),
                        content.into_string(),
                        buffer_version,
                        ctx,
                    )
                })
            }),
            Some(DiffType::Delete { .. }) => self.editor.update(ctx, |editor, ctx| {
                let buffer_version = editor.version(ctx);
                GlobalBufferModel::handle(ctx).update(ctx, move |model, ctx| {
                    model.delete(file_id, buffer_version, ctx)
                })
            }),
            _ => self.editor.update(ctx, |editor, ctx| {
                let content = editor.text(ctx);
                let buffer_version = editor.version(ctx);

                GlobalBufferModel::handle(ctx).update(ctx, move |model, ctx| {
                    model.save(file_id, content.into_string(), buffer_version, ctx)
                })
            }),
        };

        if let Err(err) = result {
            self.auto_save_in_flight = false;
            log::error!("Failed to save file: {err:?}");
            ctx.emit(LocalCodeEditorEvent::FailedToSave {
                error: Rc::new(err),
            });
        }
    }

    fn auto_save_after_delay(&mut self, ctx: &mut ViewContext<Self>) {
        if !*CodeSettings::as_ref(ctx).auto_save
            || self.diff_type.is_some()
            || !self.has_unsaved_changes(ctx)
        {
            return;
        }

        let Some(file_id) = self.file_id() else {
            return;
        };

        self.auto_save_in_flight = true;
        self.perform_save(file_id, ctx);
    }

    pub fn is_new_file(&self) -> bool {
        self.is_new_file
    }

    // This is a footgun - we should remove this codepath.
    pub fn set_new_file(&mut self, is_new: bool) {
        self.is_new_file = is_new;
    }

    pub fn set_default_directory(&mut self, directory: Option<PathBuf>) {
        self.default_directory = directory;
    }

    pub fn reset_with_state(&mut self, state: InitialBufferState, ctx: &mut ViewContext<Self>) {
        self.base_content_version = Some(state.version);
        self.editor
            .update(ctx, |editor, ctx| editor.reset(state, ctx));
    }

    /// Whether the content of the source file this editor is based on has been loaded into the buffer.
    pub fn file_loaded(&self, ctx: &mut ViewContext<Self>) -> bool {
        // For global buffer, we could have utilized a shared buffer from another open editor. To avoid
        // any race condition, we directly check with the GlobalBufferModel rather than relying on self.base_content_version
        // which is updated via an async event.
        let Some(file_id) = self.file_id() else {
            return false;
        };

        GlobalBufferModel::as_ref(ctx).buffer_loaded(file_id)
    }

    /// Construct a new local editor view with a shared buffer.
    pub fn new_with_global_buffer<T>(
        path: &Path,
        editor_constructor: T,
        enable_diff_nav_by_default: bool,
        display_mode: Option<DisplayMode>,
        ctx: &mut ViewContext<Self>,
    ) -> Self
    where
        T: FnOnce(BufferState, &mut ViewContext<Self>) -> ViewHandle<CodeEditorView>,
    {
        let buffer_state = GlobalBufferModel::handle(ctx).update(ctx, |model, ctx| {
            model.open(
                crate::code::buffer_location::BufferLocation::Local(path.to_path_buf()),
                ctx,
            )
        });
        let file_id = buffer_state.file_id;
        let editor = editor_constructor(buffer_state, ctx);

        editor.update(ctx, |editor, ctx| {
            editor.set_language_with_path(path, ctx);
            // Rebuild layout and bootstrap syntax highlighting for the editor with existing buffer content.
            editor.model.update(ctx, |model, ctx| {
                model.rebuild_layout_with_syntax_highlighting(ctx)
            });
        });

        let mut local_editor =
            Self::new(editor, None, enable_diff_nav_by_default, display_mode, ctx);

        local_editor.metadata = Some(LoadedFileMetadata::LocalFile {
            id: file_id,
            path: path.to_path_buf(),
        });

        Self::subscribe_to_global_buffer_events(file_id, ctx);

        local_editor
    }

    /// 构造一个绑定到远端 buffer 的编辑器视图。
    ///
    /// 通过 [`GlobalBufferModel::open`] 以 [`BufferLocation::Remote`] 打开远端文件,
    /// 内容由 buffer-sync 协议异步填充。语言识别复用远端路径的后缀。
    #[cfg(feature = "local_tty")]
    pub fn new_with_remote_buffer<T>(
        remote_path: crate::code::buffer_location::RemotePath,
        editor_constructor: T,
        enable_diff_nav_by_default: bool,
        display_mode: Option<DisplayMode>,
        ctx: &mut ViewContext<Self>,
    ) -> Self
    where
        T: FnOnce(BufferState, &mut ViewContext<Self>) -> ViewHandle<CodeEditorView>,
    {
        // 远端路径用于语言识别(后缀)。
        let language_path = std::path::PathBuf::from(remote_path.path.as_str());
        let buffer_state = GlobalBufferModel::handle(ctx).update(ctx, |model, ctx| {
            model.open(
                crate::code::buffer_location::BufferLocation::Remote(remote_path.clone()),
                ctx,
            )
        });
        let file_id = buffer_state.file_id;
        let editor = editor_constructor(buffer_state, ctx);

        editor.update(ctx, |editor, ctx| {
            editor.set_language_with_path(&language_path, ctx);
            editor.model.update(ctx, |model, ctx| {
                model.rebuild_layout_with_syntax_highlighting(ctx)
            });
        });

        let mut local_editor =
            Self::new(editor, None, enable_diff_nav_by_default, display_mode, ctx);

        local_editor.metadata = Some(LoadedFileMetadata::RemoteFile {
            id: file_id,
            remote_path,
        });

        Self::subscribe_to_global_buffer_events(file_id, ctx);

        local_editor
    }

    pub fn new_with_sftp_buffer<T>(
        sftp_path: crate::code::buffer_location::SftpPath,
        editor_constructor: T,
        enable_diff_nav_by_default: bool,
        display_mode: Option<DisplayMode>,
        ctx: &mut ViewContext<Self>,
    ) -> Self
    where
        T: FnOnce(BufferState, &mut ViewContext<Self>) -> ViewHandle<CodeEditorView>,
    {
        let language_path = sftp_path.path.clone();
        let buffer_state = GlobalBufferModel::handle(ctx).update(ctx, |model, ctx| {
            model.open(
                crate::code::buffer_location::BufferLocation::Sftp(sftp_path.clone()),
                ctx,
            )
        });
        let file_id = buffer_state.file_id;
        let editor = editor_constructor(buffer_state, ctx);

        editor.update(ctx, |editor, ctx| {
            editor.set_language_with_path(&language_path, ctx);
            editor.model.update(ctx, |model, ctx| {
                model.rebuild_layout_with_syntax_highlighting(ctx)
            });
        });

        let mut local_editor =
            Self::new(editor, None, enable_diff_nav_by_default, display_mode, ctx);

        local_editor.metadata = Some(LoadedFileMetadata::SftpFile {
            id: file_id,
            sftp_path,
        });

        Self::subscribe_to_global_buffer_events(file_id, ctx);

        local_editor
    }

    pub fn set_pending_scroll(&mut self, position: ScrollPosition, ctx: &mut ViewContext<Self>) {
        // If the file is already loaded, we can set the scroll trigger immediately with the
        // current buffer version. Otherwise, store it and apply when the file finishes loading.
        // This handles the race condition where set_pending_scroll is called before the file
        // content has been loaded (e.g., in the GlobalBuffer path).
        if self.file_loaded(ctx) {
            self.editor.update(ctx, |editor, ctx| {
                let version = editor.buffer_version(ctx);
                editor.set_pending_scroll(ScrollTrigger::new(position, version));
            });
        } else {
            self.pending_scroll_on_load = Some(position);
        }
    }

    fn on_file_loaded(&mut self, ctx: &mut ViewContext<Self>) {
        self.apply_diffs_if_any(ctx);
        self.file_loaded.set();

        // Apply any pending scroll position that was set before the file finished loading.
        if let Some(position) = self.pending_scroll_on_load.take() {
            self.editor.update(ctx, |editor, ctx| {
                let version = editor.buffer_version(ctx);
                editor.set_pending_scroll(ScrollTrigger::new(position, version));
            });
        }
    }

    /// Updates the enablement state of the visible "add as context" gutter button based on the file state.
    /// If the button is hidden to begin with, this is a no-op.
    pub fn update_diff_hunk_gutter_buttons(&self, ctx: &mut ViewContext<Self>) {
        let has_unsaved_changes = self.has_unsaved_changes(ctx);
        let enabled = !has_unsaved_changes;
        self.editor.update(ctx, |code_editor, ctx| {
            code_editor.set_add_diff_hunk_as_context_button(enabled, ctx);
        });
    }

    pub fn has_unsaved_changes(&self, ctx: &AppContext) -> bool {
        if self.is_new_file {
            let text = self.editor.as_ref(ctx).text(ctx);
            if text.as_str().is_empty() {
                return false;
            }
        }

        self.base_content_version
            .map(|base_version| !self.editor.as_ref(ctx).version_match(&base_version, ctx))
            .unwrap_or(false)
    }

    /// Enables the selection-as-context tooltip. For now, we only want this to be rendered within editors in code panes.
    pub(crate) fn with_selection_as_context(
        mut self,
        terminal_target_fn: Box<TerminalTargetFn>,
    ) -> Self {
        self.selection_as_context_tooltip = Some(SelectionAsContextTooltip {
            mouse_state: Default::default(),
            terminal_target_fn,
        });
        self
    }

    /// Adds the TabConfig footer to the editor view if the file is a tab config TOML.
    /// LSP 下线后,普通源码文件不再渲染 footer。
    pub(crate) fn add_footer(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(path) = self.file_path() else {
            return;
        };
        if !CodeFooterView::is_tab_config_path(path) {
            return;
        }
        let path_buf = path.to_path_buf();
        let footer = ctx.add_typed_action_view(|ctx| CodeFooterView::new(path_buf, ctx));
        ctx.subscribe_to_view(&footer, |_, _, event, ctx| match event {
            CodeFooterViewEvent::RunTabConfigSkill { path } => {
                ctx.emit(LocalCodeEditorEvent::RunTabConfigSkill { path: path.clone() });
            }
        });
        self.footer = Some(footer);
    }

    /// Unsubscribes from any existing GlobalBufferModel subscription and sets up a
    /// new one for the given `file_id`.  Handles BufferLoaded, FailedToLoad,
    /// BufferUpdatedFromFileEvent, FileSaved, and FailedToSave events.
    fn subscribe_to_global_buffer_events(file_id: FileId, ctx: &mut ViewContext<Self>) {
        ctx.unsubscribe_to_model(&GlobalBufferModel::handle(ctx));
        ctx.subscribe_to_model(&GlobalBufferModel::handle(ctx), move |me, _, event, ctx| {
            if event.file_id() != file_id {
                return;
            }
            me.update_diff_hunk_gutter_buttons(ctx);
            match event {
                GlobalBufferModelEvent::BufferLoaded {
                    content_version, ..
                } => {
                    me.sftp_save_conflict_pending = false;
                    if me.base_content_version.is_some() {
                        return;
                    }
                    me.base_content_version = Some(*content_version);
                    me.on_file_loaded(ctx);
                    ctx.emit(LocalCodeEditorEvent::FileLoaded);
                }
                GlobalBufferModelEvent::FailedToLoad { error, .. } => {
                    me.sftp_save_conflict_pending = false;
                    me.is_new_file = true;
                    me.on_file_loaded(ctx);
                    ctx.emit(LocalCodeEditorEvent::FailedToLoad {
                        error: error.clone(),
                    });
                }
                GlobalBufferModelEvent::BufferUpdatedFromFileEvent {
                    success,
                    content_version,
                    ..
                } => {
                    if !*success {
                        ctx.notify();
                    } else {
                        me.sftp_save_conflict_pending = false;
                        me.base_content_version = Some(*content_version);
                    }
                }
                GlobalBufferModelEvent::FileSaved { .. } => {
                    me.sftp_save_conflict_pending = false;
                    if let Some(sftp_path) = me.pending_sftp_save_as_path.take() {
                        me.sftp_save_as_open = false;
                        me.metadata = Some(LoadedFileMetadata::SftpFile {
                            id: file_id,
                            sftp_path: sftp_path.clone(),
                        });
                        me.editor.update(ctx, |editor, ctx| {
                            editor.set_language_with_path(&sftp_path.path, ctx);
                        });
                    }
                    let auto_saved = std::mem::take(&mut me.auto_save_in_flight);
                    ctx.emit(LocalCodeEditorEvent::FileSaved { auto_saved });
                }
                GlobalBufferModelEvent::FailedToSave { error, .. } => {
                    me.auto_save_in_flight = false;
                    me.pending_sftp_save_as_path = None;
                    me.base_content_version = GlobalBufferModel::as_ref(ctx).base_version(file_id);
                    ctx.emit(LocalCodeEditorEvent::FailedToSave {
                        error: error.clone(),
                    });
                }
                GlobalBufferModelEvent::SftpSaveConflict { .. } => {
                    me.auto_save_in_flight = false;
                    me.sftp_save_conflict_pending = true;
                    me.base_content_version = GlobalBufferModel::as_ref(ctx).base_version(file_id);
                    ctx.emit(LocalCodeEditorEvent::SftpSaveConflict);
                    ctx.notify();
                }
                // 远端 buffer 同步事件由 GlobalBufferModel / ServerModel 内部消费,
                // 本地编辑器视图不关心。
                GlobalBufferModelEvent::RemoteBufferConflict { .. }
                | GlobalBufferModelEvent::ServerLocalBufferUpdated { .. } => {}
            }
        });
    }

    pub fn has_version_conflicts(&self, app: &AppContext) -> bool {
        let Some(file_id) = self.file_id() else {
            return false;
        };
        self.has_unsaved_changes(app)
            && self.base_content_version != GlobalBufferModel::as_ref(app).base_version(file_id)
    }
    /// Save the file to the local file system.
    /// This will only return an error immediately if there is a failure in the sync part of the call.
    /// Other errors could be returned asynchronously via the FileModelEvent::FailedToSave event.
    pub fn save_local(&mut self, ctx: &mut ViewContext<Self>) -> Result<(), ImmediateSaveError> {
        let Some(file_id) = self.file_id() else {
            return Err(ImmediateSaveError::NoFileId);
        };

        // LSP 下线后不再在保存前调用 LSP format。
        self.perform_save(file_id, ctx);
        Ok(())
    }

    /// Open a save dialog to save the file with a new name, optionally with a completion callback.
    pub fn save_as(&mut self, callback: Option<SaveCallback>, ctx: &mut ViewContext<Self>) {
        ctx.open_save_file_picker(
            move |path_opt, me, ctx| Self::handle_save_as(callback, path_opt, me, ctx),
            if let Some(default_dir) = &self.default_directory {
                SaveFilePickerConfiguration::new().with_default_directory(default_dir.clone())
            } else {
                SaveFilePickerConfiguration::new()
            },
        );
    }

    fn handle_save_as(
        callback: Option<SaveCallback>,
        path_opt: Option<String>,
        me: &mut Self,
        ctx: &mut ViewContext<Self>,
    ) {
        let callback = callback.unwrap_or(Box::new(|_, _| {}));
        let Some(path_str) = path_opt else {
            callback(SaveOutcome::Canceled, ctx);
            return;
        };
        let path = PathBuf::from(path_str);

        // Ensure parent directories exist before registering file watcher / LSP.
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }

        let buffer = me.editor.as_ref(ctx).model.as_ref(ctx).buffer().clone();
        let buffer_state = GlobalBufferModel::handle(ctx)
            .update(ctx, |model, ctx| model.register(path.clone(), buffer, ctx));

        let file_id = buffer_state.file_id;
        me.metadata = Some(LoadedFileMetadata::LocalFile {
            id: file_id,
            path: path.clone(),
        });

        me.set_new_file(false);

        me.editor.update(ctx, |editor, ctx| {
            editor.set_language_with_path(&path, ctx);
        });

        let content = me.editor.as_ref(ctx).text(ctx).into_string();
        let buffer_version = me.editor.as_ref(ctx).version(ctx);

        me.base_content_version = Some(buffer_version);
        let save_outcome = if let Err(err) = GlobalBufferModel::handle(ctx)
            .update(ctx, move |model, ctx| {
                model.save(file_id, content, buffer_version, ctx)
            }) {
            log::error!("Failed to save file to new path: {err:?}");
            ctx.emit(LocalCodeEditorEvent::FailedToSave {
                error: Rc::new(err),
            });
            SaveOutcome::Failed
        } else {
            Self::subscribe_to_global_buffer_events(file_id, ctx);
            SaveOutcome::Succeeded
        };
        callback(save_outcome, ctx);
    }

    pub fn cursor_at(&self, point: Point, ctx: &mut ViewContext<Self>) {
        self.editor.update(ctx, |editor, ctx| {
            editor.cursor_at(point, ctx);
        });
    }

    /// If there is a pending diff available, apply it on the buffer. This should only be called _after_ the buffer
    /// has been loaded.
    fn apply_diffs_if_any(&mut self, ctx: &mut ViewContext<Self>) -> Option<usize> {
        let diff = self.diff_type.clone()?;
        let deltas = match diff {
            DiffType::Create { delta } => vec![delta],
            DiffType::Update { mut deltas, .. } => {
                deltas.sort_by_key(|delta| delta.replacement_line_range.start);
                deltas
            }
            DiffType::Delete { delta } => vec![delta],
        };

        // Early return if the pending diff itself is empty.
        let first_line_start = deltas
            .first()
            .map(|diff| diff.replacement_line_range.start)?;

        self.editor.update(ctx, |editor, ctx| {
            editor.apply_diffs(deltas, ctx);

            if self.enable_diff_nav_by_default {
                editor.toggle_diff_nav(None, ctx);
            }
        });

        Some(first_line_start)
    }

    pub fn file_id(&self) -> Option<FileId> {
        self.metadata.as_ref().map(|metadata| match metadata {
            LoadedFileMetadata::LocalFile { id, .. }
            | LoadedFileMetadata::RemoteFile { id, .. }
            | LoadedFileMetadata::SftpFile { id, .. } => *id,
        })
    }

    pub fn file_path(&self) -> Option<&Path> {
        match self.metadata.as_ref()? {
            LoadedFileMetadata::LocalFile { path, .. } => Some(path.as_path()),
            // 远端文件没有本地路径。
            LoadedFileMetadata::RemoteFile { .. } | LoadedFileMetadata::SftpFile { .. } => None,
        }
    }

    pub fn sftp_path(&self) -> Option<SftpPath> {
        match self.metadata.as_ref()? {
            LoadedFileMetadata::SftpFile { sftp_path, .. } => Some(sftp_path.clone()),
            LoadedFileMetadata::LocalFile { .. } | LoadedFileMetadata::RemoteFile { .. } => None,
        }
    }

    /// Update this editor's file identity after a `GlobalBufferModel::rename`.
    /// Sets the new file_id and path, re-subscribes to `GlobalBufferModelEvent`,
    /// and updates the language from the new path.
    #[cfg(feature = "local_fs")]
    pub fn apply_rename(
        &mut self,
        buffer_state: BufferState,
        new_path: &Path,
        ctx: &mut ViewContext<Self>,
    ) {
        let file_id = buffer_state.file_id;
        self.metadata = Some(LoadedFileMetadata::LocalFile {
            id: file_id,
            path: new_path.to_path_buf(),
        });

        self.editor.update(ctx, |editor, ctx| {
            editor.set_language_with_path(new_path, ctx);
        });

        // Re-subscribe to GlobalBufferModel events for the new file_id.
        Self::subscribe_to_global_buffer_events(file_id, ctx);
    }

    pub fn editor(&self) -> &ViewHandle<CodeEditorView> {
        &self.editor
    }

    /// Accept the diff that is currently in the editor. For local files, this can only be called after the file contents
    /// have been loaded into the editor.
    /// If it is a local file, the diff content will be retrieved and the pending diff will be marked as completed.
    /// If it is not a local file, the pending diff will be marked as completed with an empty diff.
    pub fn accept_diff(&mut self, ctx: &mut ViewContext<Self>) {
        match self.file_path() {
            Some(file) => {
                // Begin calculating the diff that will be saved.  When the result comes back, the diff will be marked completed.
                self.editor.update(ctx, |view, ctx| {
                    view.retrieve_unified_diff(file.display().to_string(), ctx)
                });
            }
            None => {
                ctx.emit(LocalCodeEditorEvent::DiffAccepted);
            }
        };
    }

    pub fn close_find_bar(&mut self, should_focus_editor: bool, ctx: &mut ViewContext<Self>) {
        self.editor.update(ctx, |editor, ctx| {
            editor.close_find_bar(should_focus_editor, ctx);
        });
    }

    /// If a single terminal view exists in the active window, returns the active file path's relative to to the terminal's session.
    fn file_path_relative_to_terminal_view(&self, app: &AppContext) -> Option<String> {
        if let Some(terminal_target_fn) = self
            .selection_as_context_tooltip
            .as_ref()
            .map(|tooltip| &tooltip.terminal_target_fn)
        {
            app.windows().active_window().and_then(|window_id| {
                terminal_target_fn(window_id, app).and_then(|terminal_view| {
                    terminal_view
                        .as_ref(app)
                        .active_session_path_if_local(app)
                        .and_then(|cwd| {
                            let is_wsl = terminal_view
                                .as_ref(app)
                                .active_session_wsl_distro(app)
                                .is_some();
                            self.file_path()
                                .and_then(|file_path| to_relative_path(is_wsl, file_path, &cwd))
                        })
                })
            })
        } else {
            None
        }
    }

    fn render_selection_tooltip(&self, app: &AppContext) -> Option<Box<dyn Element>> {
        // If there's a single selection and an active terminal view, we want to give the user an option to add the selection as context.
        self.selection_as_context_tooltip
            .as_ref()
            .and_then(|selection_as_context_tooltip| {
                if self.editor.as_ref(app).selected_lines(app).is_some()
                    && self.file_path_relative_to_terminal_view(app).is_some()
                {
                    let appearance = Appearance::as_ref(app);
                    let theme = appearance.theme();
                    let modifier_keys = if cfg!(target_os = "macos") {
                        "⌘L"
                    } else {
                        "Ctrl-L"
                    };

                    let mut row = Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_main_axis_alignment(MainAxisAlignment::Center)
                        .with_main_axis_size(MainAxisSize::Min);
                    row.add_child(
                        Shrinkable::new(
                            1.,
                            Text::new_inline(
                                "Add as context",
                                appearance.ui_font_family(),
                                appearance.ui_font_size(),
                            )
                            .with_color(theme.active_ui_text_color().into())
                            .finish(),
                        )
                        .finish(),
                    );
                    row.add_child(
                        Container::new(
                            Text::new_inline(
                                modifier_keys,
                                appearance.ui_font_family(),
                                appearance.ui_font_overline(),
                            )
                            .with_color(theme.disabled_ui_text_color().into())
                            .finish(),
                        )
                        .with_margin_left(8.)
                        .finish(),
                    );

                    Some(
                        Hoverable::new(selection_as_context_tooltip.mouse_state.clone(), |state| {
                            let background_color = if state.is_hovered() {
                                theme.surface_2()
                            } else {
                                theme.surface_1()
                            };
                            let internal_container = Container::new(row.finish())
                                .with_padding_left(12.)
                                .with_padding_right(12.)
                                .with_padding_top(4.)
                                .with_padding_bottom(4.)
                                .finish();
                            Container::new(internal_container)
                                .with_background(background_color)
                                .with_padding_top(4.)
                                .with_padding_bottom(4.)
                                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
                                .with_border(Border::all(1.5).with_border_fill(theme.surface_2()))
                                .with_drop_shadow(DropShadow::new_with_standard_offset_and_spread(
                                    DROP_SHADOW_COLOR,
                                ))
                                .finish()
                        })
                        .on_click(move |ctx, _app, _pos| {
                            ctx.dispatch_typed_action(
                                LocalCodeEditorAction::InsertSelectedTextToInput,
                            );
                        })
                        .finish(),
                    )
                } else {
                    None
                }
            })
    }

    fn insert_selected_text_to_input(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(relative_file_path) = self.file_path_relative_to_terminal_view(ctx) else {
            return;
        };

        let mut line_range: Option<Range<LineCount>> = None;
        let mut selected_text: Option<String> = None;
        self.editor.update(ctx, |editor, ctx| {
            // If we have a vim visual selection, update the editor model to use that as a selection range
            let has_vim_visual = matches!(editor.vim_mode(ctx), Some(VimMode::Visual(_)));
            if has_vim_visual {
                editor.model.update(ctx, |model, ctx| {
                    model.vim_visual_selection_range(MotionType::Linewise, false, ctx);
                });
            }

            if let Some((start, end)) = editor.selected_lines(ctx) {
                // selected_lines() returns 1-indexed row numbers.
                line_range = Some(LineCount::from(start as usize)..LineCount::from(end as usize));
                selected_text = Some(editor.selected_text(ctx).unwrap_or_default());
            }

            // Enter normal mode
            if has_vim_visual {
                editor.enter_vim_normal_mode(ctx);
            }
        });

        let (Some(line_range), Some(selected_text)) = (line_range, selected_text) else {
            return;
        };

        ctx.emit(LocalCodeEditorEvent::SelectionAddedAsContext {
            relative_file_path,
            line_range,
            selected_text,
        });
        self.editor.update(ctx, |editor, ctx| {
            editor.clear_selection(ctx);
        });
    }

    pub fn diff(&self) -> Option<&DiffType> {
        self.diff_type.as_ref()
    }
}

impl DiffViewer for LocalCodeEditorView {
    fn editor(&self) -> &ViewHandle<CodeEditorView> {
        &self.editor
    }

    fn diff(&self) -> Option<&DiffType> {
        self.diff_type.as_ref()
    }

    fn was_edited(&self) -> bool {
        self.was_edited
    }

    /// Automatically accept and save this diff. Unlike [`Self::accept_diff`] and [`Self::save_local`], this
    /// waits for the initial file contents to be loaded.
    fn accept_and_save_diff(&self, ctx: &mut ViewContext<Self>) {
        ctx.spawn(self.file_loaded.wait(), move |me, _, ctx| {
            me.accept_diff(ctx);
            if let Err(err) = me.save_local(ctx) {
                log::error!("{err:?}");
                if let ImmediateSaveError::FailedToSave(err) = err {
                    ctx.emit(LocalCodeEditorEvent::FailedToSave {
                        error: Rc::new(err),
                    });
                }
            }
        });
    }

    fn reject_diff(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.emit(LocalCodeEditorEvent::DiffRejected);
    }

    fn restore_diff_base(&mut self, ctx: &mut ViewContext<Self>) -> Result<(), String> {
        if self.is_new_file {
            if let Some(file_id) = self.file_id() {
                GlobalBufferModel::handle(ctx).update(ctx, |model, ctx| {
                    model.remove(file_id, ctx);
                });
            }
            if let Some(path) = self.file_path().map(|p| p.to_path_buf()) {
                if let Err(e) = std::fs::remove_file(&path) {
                    log::error!("Failed to delete file after save: {e}");
                } else {
                    // This will close tabs with the file open
                    ctx.dispatch_typed_action(&WorkspaceAction::FileDeleted { path });
                }
            }

            return Ok(());
        }

        let base_content = self
            .editor
            .as_ref(ctx)
            .model
            .as_ref(ctx)
            .diff()
            .as_ref(ctx)
            .base()
            .ok_or_else(|| "Missing base content".to_string())?
            .to_string();

        let file_id = self
            .file_id()
            .ok_or_else(|| "Missing file_id".to_string())?;

        let buffer_version = self.editor.as_ref(ctx).version(ctx);

        GlobalBufferModel::handle(ctx)
            .update(ctx, |model, ctx| {
                model.save(file_id, base_content, buffer_version, ctx)
            })
            .map_err(|e| format!("Failed to save file: {e:?}"))
    }
}

impl Entity for LocalCodeEditorView {
    type Event = LocalCodeEditorEvent;
}

impl View for LocalCodeEditorView {
    fn ui_name() -> &'static str {
        "LocalCodeEditorView"
    }

    fn on_focus(&mut self, focus_ctx: &warpui::FocusContext, ctx: &mut ViewContext<Self>) {
        if focus_ctx.is_self_focused() {
            self.editor.update(ctx, |editor, ctx| editor.focus(ctx));
        }
    }

    fn render(&self, app: &AppContext) -> Box<dyn warpui::Element> {
        // Rendering the version conflict banner.
        let base: Box<dyn Element> = if self.sftp_save_conflict_pending {
            let appearance = Appearance::as_ref(app);
            let banner = render_sftp_save_conflict_banner(
                appearance,
                self.conflict_banner_mouse_states
                    .reload_sftp_mouse_state
                    .clone(),
                self.conflict_banner_mouse_states
                    .force_save_sftp_mouse_state
                    .clone(),
                self.conflict_banner_mouse_states
                    .save_sftp_as_mouse_state
                    .clone(),
                self.conflict_banner_mouse_states
                    .cancel_sftp_conflict_mouse_state
                    .clone(),
            );
            let mut col = Flex::column().with_child(banner);
            if self.sftp_save_as_open {
                col.add_child(render_sftp_save_as_path_input(
                    appearance,
                    &self.sftp_save_as_path_editor,
                    self.conflict_banner_mouse_states
                        .confirm_sftp_save_as_mouse_state
                        .clone(),
                ));
            }

            let editor_view = ChildView::new(&self.editor).finish();
            if self.editor.as_ref(app).needs_vertical_constraint() {
                col.add_child(Shrinkable::new(1., editor_view).finish());
            } else {
                col.add_child(editor_view);
            }
            col.finish()
        } else if self.has_version_conflicts(app) {
            let appearance = Appearance::as_ref(app);
            let banner = render_unsaved_changes_banner(
                appearance,
                self.conflict_banner_mouse_states
                    .discard_mouse_state
                    .clone(),
                self.conflict_banner_mouse_states
                    .overwrite_mouse_state
                    .clone(),
            );
            let mut col = Flex::column().with_child(banner);

            let editor_view = ChildView::new(&self.editor).finish();
            if self.editor.as_ref(app).needs_vertical_constraint() {
                col.add_child(Shrinkable::new(1., editor_view).finish());
            } else {
                col.add_child(editor_view);
            }
            col.finish()
        } else {
            ChildView::new(&self.editor).finish()
        };

        let base_with_handler = base;

        let mut stack = Stack::new()
            .with_constrain_absolute_children()
            .with_child(base_with_handler);

        let editor = self.editor().as_ref(app);
        if self.selection_as_context_tooltip.is_some() {
            // When a single terminal exists in the window and the user has made a selection (but isn't currently selecting),
            // we render a tooltip that allows them to add the selected text to the terminal context.
            let is_ai_enabled = AISettings::as_ref(app).is_any_ai_enabled(app);
            if is_ai_enabled
                && FeatureFlag::SelectionAsContext.is_enabled()
                && !editor.is_selecting()
            {
                let tooltip = self.render_selection_tooltip(app);
                if let Some(tooltip) = tooltip {
                    stack.add_positioned_child(tooltip, editor.selection_position_anchor(app))
                }
            }
        }

        if let Some(footer) = &self.footer {
            let mut col = Flex::column();

            if self.editor.as_ref(app).needs_vertical_constraint() {
                col.add_child(Shrinkable::new(1., stack.finish()).finish());
            } else {
                col.add_child(stack.finish());
            }
            col.with_child(ChildView::new(footer).finish()).finish()
        } else {
            stack.finish()
        }
    }
}

impl TypedActionView for LocalCodeEditorView {
    type Action = LocalCodeEditorAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            LocalCodeEditorAction::InsertSelectedTextToInput => {
                self.insert_selected_text_to_input(ctx);
            }
            LocalCodeEditorAction::SaveFile => {
                if let Err(ImmediateSaveError::FailedToSave(err)) = self.save_local(ctx) {
                    log::error!("Failed to save file {err:?}");
                    ctx.emit(LocalCodeEditorEvent::FailedToSave {
                        error: Rc::new(err),
                    });
                };
            }
            LocalCodeEditorAction::DiscardUnsavedChanges => {
                if let Some(path) = self.file_path().map(Path::to_path_buf) {
                    self.base_content_version = Some(self.editor().as_ref(ctx).version(ctx));
                    ctx.emit(LocalCodeEditorEvent::DiscardUnsavedChanges { path });
                }
            }
            LocalCodeEditorAction::ReloadSftpFromRemote => {
                let Some(file_id) = self.file_id() else {
                    return;
                };
                GlobalBufferModel::handle(ctx)
                    .update(ctx, |model, ctx| model.reload_sftp_buffer(file_id, ctx));
            }
            LocalCodeEditorAction::ForceSaveSftp => {
                let Some(file_id) = self.file_id() else {
                    return;
                };
                GlobalBufferModel::handle(ctx)
                    .update(ctx, |model, ctx| model.force_save_sftp_buffer(file_id, ctx));
            }
            LocalCodeEditorAction::SaveSftpAs => {
                self.begin_sftp_save_as(ctx);
            }
            LocalCodeEditorAction::ConfirmSaveSftpAs => {
                self.confirm_sftp_save_as(ctx);
            }
            LocalCodeEditorAction::CancelSftpConflict => {
                self.sftp_save_conflict_pending = false;
                self.sftp_save_as_open = false;
                self.pending_sftp_save_as_path = None;
                ctx.notify();
            }
        }
    }
}

pub fn render_sftp_save_conflict_banner(
    appearance: &Appearance,
    reload_mouse_state: MouseStateHandle,
    overwrite_mouse_state: MouseStateHandle,
    save_as_mouse_state: MouseStateHandle,
    cancel_mouse_state: MouseStateHandle,
) -> Box<dyn Element> {
    let left = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(
            Container::new(
                ConstrainedBox::new(
                    Icon::Warning
                        .to_warpui_icon(appearance.theme().active_ui_text_color())
                        .finish(),
                )
                .with_height(16.)
                .with_width(16.)
                .finish(),
            )
            .with_margin_right(8.)
            .finish(),
        )
        .with_child(
            Shrinkable::new(
                1.,
                Text::new(
                    "远程文件已变化，当前编辑尚未保存。",
                    appearance.ui_font_family(),
                    appearance.ui_font_size(),
                )
                .with_color(appearance.theme().active_ui_text_color().into())
                .soft_wrap(true)
                .finish(),
            )
            .finish(),
        )
        .finish();

    let text_button =
        |label: &str, mouse_state: MouseStateHandle, action: LocalCodeEditorAction| {
            appearance
                .ui_builder()
                .button(ButtonVariant::Text, mouse_state)
                .with_text_label(label.to_string())
                .with_style(UiComponentStyles {
                    height: Some(24.),
                    padding: Some(Coords {
                        left: 8.,
                        right: 8.,
                        ..Default::default()
                    }),
                    font_color: Some(appearance.theme().active_ui_text_color().into()),
                    ..Default::default()
                })
                .build()
                .on_click(move |ctx, _, _| ctx.dispatch_typed_action(action.clone()))
                .finish()
        };

    let right = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(text_button(
            "重新加载",
            reload_mouse_state,
            LocalCodeEditorAction::ReloadSftpFromRemote,
        ))
        .with_child(text_button(
            "另存为",
            save_as_mouse_state,
            LocalCodeEditorAction::SaveSftpAs,
        ))
        .with_child(text_button(
            "取消",
            cancel_mouse_state,
            LocalCodeEditorAction::CancelSftpConflict,
        ))
        .with_child(
            Container::new(
                appearance
                    .ui_builder()
                    .button(ButtonVariant::Outlined, overwrite_mouse_state)
                    .with_text_label("覆盖保存".to_string())
                    .with_style(UiComponentStyles {
                        font_color: Some(appearance.theme().active_ui_text_color().into()),
                        ..Default::default()
                    })
                    .build()
                    .on_click(move |ctx, _, _| {
                        ctx.dispatch_typed_action(LocalCodeEditorAction::ForceSaveSftp)
                    })
                    .finish(),
            )
            .with_margin_left(4.)
            .finish(),
        )
        .finish();

    Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(Shrinkable::new(1., left).finish())
            .with_child(right)
            .finish(),
    )
    .with_background(appearance.theme().text_selection_as_context_color())
    .with_padding_top(4.)
    .with_padding_bottom(4.)
    .with_padding_left(12.)
    .with_padding_right(12.)
    .finish()
}

pub fn render_sftp_save_as_path_input(
    appearance: &Appearance,
    editor: &ViewHandle<EditorView>,
    confirm_mouse_state: MouseStateHandle,
) -> Box<dyn Element> {
    let editor_el = Container::new(
        Shrinkable::new(1., Clipped::new(ChildView::new(editor).finish()).finish()).finish(),
    )
    .with_padding_left(8.)
    .with_padding_right(8.)
    .with_padding_top(4.)
    .with_padding_bottom(4.)
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
    .with_background(appearance.theme().surface_2())
    .finish();

    Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_size(MainAxisSize::Max)
            .with_spacing(8.)
            .with_child(Shrinkable::new(1., editor_el).finish())
            .with_child(
                appearance
                    .ui_builder()
                    .button(ButtonVariant::Outlined, confirm_mouse_state)
                    .with_text_label("保存副本".to_string())
                    .with_style(UiComponentStyles {
                        font_color: Some(appearance.theme().active_ui_text_color().into()),
                        ..Default::default()
                    })
                    .build()
                    .on_click(move |ctx, _, _| {
                        ctx.dispatch_typed_action(LocalCodeEditorAction::ConfirmSaveSftpAs)
                    })
                    .finish(),
            )
            .finish(),
    )
    .with_background(appearance.theme().text_selection_as_context_color())
    .with_padding_left(12.)
    .with_padding_right(12.)
    .with_padding_bottom(8.)
    .finish()
}

/// Renders a banner warning that the file has saved changes not reflected in the diff
pub fn render_unsaved_changes_banner(
    appearance: &Appearance,
    discard_mouse_state: MouseStateHandle,
    overwrite_mouse_state: MouseStateHandle,
) -> Box<dyn Element> {
    let left = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(
            Container::new(
                ConstrainedBox::new(
                    Icon::Warning
                        .to_warpui_icon(appearance.theme().active_ui_text_color())
                        .finish(),
                )
                .with_height(16.)
                .with_width(16.)
                .finish(),
            )
            .with_margin_right(8.)
            .finish(),
        )
        .with_child(
            Shrinkable::new(
                1.,
                Text::new(
                    "This file has saved changes that are not reflected here.",
                    appearance.ui_font_family(),
                    appearance.ui_font_size(),
                )
                .with_color(appearance.theme().active_ui_text_color().into())
                .soft_wrap(true)
                .finish(),
            )
            .finish(),
        )
        .finish();

    let right = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(
            appearance
                .ui_builder()
                .button(ButtonVariant::Text, discard_mouse_state)
                .with_text_label(crate::t!("code-discard-this-version"))
                .with_style(UiComponentStyles {
                    height: Some(24.),
                    padding: Some(Coords {
                        left: 8.,
                        right: 8.,
                        ..Default::default()
                    }),
                    font_color: Some(appearance.theme().active_ui_text_color().into()),
                    ..Default::default()
                })
                .build()
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(LocalCodeEditorAction::DiscardUnsavedChanges)
                })
                .finish(),
        )
        .with_child(
            Container::new(
                appearance
                    .ui_builder()
                    .button(ButtonVariant::Outlined, overwrite_mouse_state)
                    .with_text_label(crate::t!("code-overwrite"))
                    .with_style(UiComponentStyles {
                        font_color: Some(appearance.theme().active_ui_text_color().into()),
                        ..Default::default()
                    })
                    .build()
                    .on_click(move |ctx, _, _| {
                        ctx.dispatch_typed_action(LocalCodeEditorAction::SaveFile)
                    })
                    .finish(),
            )
            .with_margin_left(4.)
            .finish(),
        )
        .finish();

    Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(Shrinkable::new(1., left).finish())
            .with_child(right)
            .finish(),
    )
    .with_background(appearance.theme().text_selection_as_context_color())
    .with_padding_top(4.)
    .with_padding_bottom(4.)
    .with_padding_left(12.)
    .with_padding_right(12.)
    .finish()
}

/// Renders a small yellow circle with tooltip indicating unsaved changes
pub fn render_unsaved_circle_with_tooltip(
    mouse_state: MouseStateHandle,
    tooltip_text: String,
    size: f32,
    right_margin: f32,
    appearance: &Appearance,
) -> Box<dyn Element> {
    Hoverable::new(mouse_state, |state| {
        let rect = Container::new(
            ConstrainedBox::new(
                Rect::new()
                    .with_background_color(appearance.theme().active_ui_text_color().into())
                    .with_corner_radius(CornerRadius::with_all(Radius::Percentage(50.)))
                    .finish(),
            )
            .with_width(size)
            .with_height(size)
            .finish(),
        )
        .with_margin_right(right_margin)
        .finish();

        if state.is_hovered() {
            let mut stack = Stack::new().with_child(rect);

            let tooltip = appearance
                .ui_builder()
                .tool_tip(tooltip_text)
                .build()
                .finish();

            stack.add_positioned_overlay_child(
                tooltip,
                OffsetPositioning::offset_from_parent(
                    Vector2F::new(0., 4.),
                    ParentOffsetBounds::Unbounded,
                    ParentAnchor::BottomMiddle,
                    ChildAnchor::TopMiddle,
                ),
            );
            stack.finish()
        } else {
            rect
        }
    })
    .finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_sftp_save_as_path_keeps_remote_absolute_paths() {
        let resolved = LocalCodeEditorView::resolve_sftp_save_as_path(
            Path::new("/home/me/a.txt"),
            "/tmp/b.txt",
        )
        .expect("绝对远程路径应有效");

        assert_eq!(resolved, PathBuf::from("/tmp/b.txt"));
    }

    #[test]
    fn resolve_sftp_save_as_path_resolves_relative_to_current_parent() {
        let resolved =
            LocalCodeEditorView::resolve_sftp_save_as_path(Path::new("/home/me/a.txt"), "b.txt")
                .expect("相对远程路径应有效");

        assert_eq!(resolved, PathBuf::from("/home/me/b.txt"));
    }

    #[test]
    fn resolve_sftp_save_as_path_rejects_parent_traversal() {
        assert_eq!(
            LocalCodeEditorView::resolve_sftp_save_as_path(Path::new("/home/me/a.txt"), "../b.txt"),
            None
        );
        assert_eq!(
            LocalCodeEditorView::resolve_sftp_save_as_path(
                Path::new("/home/me/a.txt"),
                "/../b.txt"
            ),
            None
        );
    }

    #[test]
    fn suggested_sftp_save_as_path_inserts_copy_before_extension() {
        assert_eq!(
            LocalCodeEditorView::suggested_sftp_save_as_path(Path::new("/home/me/app.toml")),
            PathBuf::from("/home/me/app.copy.toml")
        );
    }
}
