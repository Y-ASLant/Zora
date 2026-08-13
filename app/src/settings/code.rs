use settings::{
    macros::define_settings_group, RespectUserSyncSetting, SupportedPlatforms, SyncToCloud,
};

pub const REMOTE_FILE_AUTO_OPEN_TEXT_MIN_MIB: u64 = 1;
pub const REMOTE_FILE_AUTO_OPEN_TEXT_MAX_MIB: u64 = 64;
pub const REMOTE_FILE_TEXT_CACHE_MIN_MIB: u64 = 0;
pub const REMOTE_FILE_TEXT_CACHE_MAX_MIB: u64 = 512;
pub const REMOTE_FILE_LARGE_PREVIEW_MIN_KIB: u64 = 256;
pub const REMOTE_FILE_LARGE_PREVIEW_MAX_KIB: u64 = 8 * 1024;

define_settings_group!(CodeSettings, settings: [
    code_as_default_editor: CodeAsDefaultEditor {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Never,
        private: false,
        toml_path: "code.editor.use_warp_as_default_editor",
        description: "Whether Zora is used as the default code editor.",
    }

    // Whether or not the user has manually dismissed the code toolbelt new feature popup.
    dismissed_code_toolbelt_new_feature_popup: DismissedCodeToolbeltNewFeaturePopup {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: true,
    },
    // Controls whether the project explorer / file tree appears in the tools panel.
    show_project_explorer: ShowProjectExplorer {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "code.editor.show_project_explorer",
        description: "Whether the project explorer is shown in the tools panel.",
    },
    // Controls whether global file search appears in the tools panel.
    show_global_search: ShowGlobalSearch {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "code.editor.show_global_search",
        description: "Whether global file search is shown in the tools panel.",
    },
    show_hidden_files: ShowHiddenFiles {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "code.editor.show_hidden_files",
        description: "Whether hidden files are shown in the project explorer.",
    },
    show_line_numbers: ShowLineNumbers {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "code.editor.show_line_numbers",
        description: "Whether line numbers are shown in the code editor.",
    },
    auto_save: AutoSave {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "code.editor.auto_save",
        description: "Whether the code editor saves file changes after typing stops.",
    },
    remote_file_auto_open_text_max_mib: RemoteFileAutoOpenTextMaxMiB {
        type: u64,
        default: 8,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "code.remote_files.auto_open_text_max_mib",
        description: "Maximum remote text file size, in MiB, to automatically open in Zora.",
    },
    remote_file_text_cache_max_mib: RemoteFileTextCacheMaxMiB {
        type: u64,
        default: 64,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "code.remote_files.text_cache_max_mib",
        description: "Maximum in-memory cache size, in MiB, for remote text files. 0 disables the cache.",
    },
    remote_file_large_preview_max_kib: RemoteFileLargePreviewMaxKiB {
        type: u64,
        default: 1024,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "code.remote_files.large_preview_max_kib",
        description: "Maximum prefix size, in KiB, used for large remote file previews.",
    },
    remote_file_external_auto_upload: RemoteFileExternalAutoUpload {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "code.remote_files.external_auto_upload",
        description: "Whether externally opened remote files should be uploaded after external edits. Defaults off.",
    },
]);

fn mib_to_bytes(value: u64) -> u64 {
    value.saturating_mul(1024).saturating_mul(1024)
}

fn kib_to_bytes(value: u64) -> u64 {
    value.saturating_mul(1024)
}

impl CodeSettings {
    pub fn remote_file_auto_open_text_max_bytes(&self) -> u64 {
        mib_to_bytes((*self.remote_file_auto_open_text_max_mib).clamp(
            REMOTE_FILE_AUTO_OPEN_TEXT_MIN_MIB,
            REMOTE_FILE_AUTO_OPEN_TEXT_MAX_MIB,
        ))
    }

    pub fn remote_file_text_cache_max_bytes(&self) -> u64 {
        mib_to_bytes((*self.remote_file_text_cache_max_mib).clamp(
            REMOTE_FILE_TEXT_CACHE_MIN_MIB,
            REMOTE_FILE_TEXT_CACHE_MAX_MIB,
        ))
    }

    pub fn remote_file_large_preview_max_bytes(&self) -> u64 {
        kib_to_bytes((*self.remote_file_large_preview_max_kib).clamp(
            REMOTE_FILE_LARGE_PREVIEW_MIN_KIB,
            REMOTE_FILE_LARGE_PREVIEW_MAX_KIB,
        ))
    }
}
