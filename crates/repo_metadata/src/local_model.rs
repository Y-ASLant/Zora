#![cfg_attr(not(feature = "local_fs"), allow(dead_code))]
//! Repository metadata model singleton.
//!
//! This module provides a singleton model that manages repository metadata across
//! all repositories tracked by Zap.

use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    rc::Rc,
};

#[cfg(feature = "local_fs")]
use std::sync::OnceLock;

use futures::{channel::oneshot, future::BoxFuture, FutureExt as _};
use warp_core::{safe_warn, send_telemetry_from_ctx};
use warpui::{
    r#async::{FutureId, SpawnedFutureHandle},
    ModelHandle,
};

/// Represents either a file or directory in a repository.
#[derive(Debug, Clone)]
pub enum RepoContent<'a> {
    File(&'a FileTreeFileMetadata),
    Directory(&'a FileTreeDirectoryEntryState),
}

use warp_util::standardized_path::StandardizedPath;

#[cfg(feature = "local_fs")]
use crate::{add_gitignores_for_path, gitignores_for_directory, matches_gitignores};
use crate::{
    entry::{Entry, FileId, IgnoredPathStrategy},
    repository::Repository,
    telemetry::RepoMetadataTelemetryEvent,
    RepoMetadataError,
};
use std::sync::Arc;
cfg_if::cfg_if! {
    if #[cfg(feature = "local_fs")] {
        use notify_debouncer_full::notify::RecursiveMode;
        use crate::repositories::{DetectedRepositories, DetectedRepositoriesEvent};
        use watcher::{BulkFilesystemWatcher, BulkFilesystemWatcherEvent};
        use warpui::SingletonEntity as _;

        /// Duration between filesystem watch events in seconds
        const FILESYSTEM_WATCHER_DEBOUNCE_SECS: u64 = 1;
    }
}

use crate::file_tree_store::{
    FileTreeDirectoryEntryState, FileTreeEntry, FileTreeEntryState, FileTreeFileMetadata,
    FileTreeState,
};
use crate::file_tree_update::{
    flatten_entry_metadata, DirectoryNodeMetadata, FileNodeMetadata, FileTreeEntryUpdate,
    RepoMetadataUpdate, RepoNodeMetadata,
};
use ignore::gitignore::Gitignore;
use warpui::ModelContext;

#[cfg(feature = "local_fs")]
const MAX_CONCURRENT_TREE_SCANS: usize = 2;

/// 限制同时执行的目录扫描数，避免大量 watcher 事件在后台同时抢占磁盘、CPU
/// 和防病毒扫描资源。令牌在 future 被取消时自动归还。
#[cfg(feature = "local_fs")]
struct TreeScanPermit {
    available: async_channel::Sender<()>,
}

#[cfg(feature = "local_fs")]
impl Drop for TreeScanPermit {
    fn drop(&mut self) {
        let _ = self.available.try_send(());
    }
}

#[cfg(feature = "local_fs")]
fn tree_scan_limiter() -> &'static (async_channel::Sender<()>, async_channel::Receiver<()>) {
    static LIMITER: OnceLock<(async_channel::Sender<()>, async_channel::Receiver<()>)> =
        OnceLock::new();
    LIMITER.get_or_init(|| {
        let (sender, receiver) = async_channel::bounded(MAX_CONCURRENT_TREE_SCANS);
        for _ in 0..MAX_CONCURRENT_TREE_SCANS {
            sender
                .try_send(())
                .expect("Tree scan limiter must accept its initial permits");
        }
        (sender, receiver)
    })
}

#[cfg(feature = "local_fs")]
async fn acquire_tree_scan_permit() -> TreeScanPermit {
    let (available, permits) = tree_scan_limiter();
    permits
        .recv()
        .await
        .expect("Tree scan limiter sender must remain alive");
    TreeScanPermit {
        available: available.clone(),
    }
}

/// Maximum depth to traverse when building file trees
const MAX_TREE_DEPTH: usize = 200;

/// Maximum number of files to index per repository to guard against really large codebases
const MAX_FILES_PER_REPO: usize = 100_000;

/// Returns true when `path` is too broad to be a recursive file-watch root.
///
/// Rejects the user's home directory itself and any of its ancestors
/// (e.g. `/Users`, `/home`, `/`). Registering such a path as a repository
/// root makes the OS push fsevents from unrelated areas (`~/Library/*`,
/// `~/Pictures/Photos Library.photoslibrary/*`, IM caches, …) into the
/// indexer, leaking user data and producing endless `PermissionDenied`
/// build_tree noise.
#[cfg(feature = "local_fs")]
fn is_unsafe_watch_root(path: &Path) -> bool {
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    path == home.as_path() || home.starts_with(path)
}

#[derive(Debug)]
/// Events emitted by the LocalRepoMetadataModel.
pub enum RepositoryMetadataEvent {
    /// A repository was added or updated.
    RepositoryUpdated {
        path: StandardizedPath,
    },
    /// A repository was removed.
    RepositoryRemoved {
        path: StandardizedPath,
    },
    /// The file tree for the repositories were updated.
    FileTreeUpdated {
        paths: Vec<StandardizedPath>,
    },
    /// The file tree's [`Entry`] was updated.
    FileTreeEntryUpdated {
        path: StandardizedPath,
    },
    UpdatingRepositoryFailed {
        path: StandardizedPath,
    },
    /// Emitted after watcher mutations are applied when
    /// `emit_incremental_updates` is enabled, containing a serializable
    /// update suitable for sending to the remote client.
    IncrementalUpdateReady {
        update: RepoMetadataUpdate,
    },
}

/// Represents the state of a repository in the metadata model.
#[derive(Debug)]
pub enum IndexedRepoState {
    /// Repository is currently being indexed.
    Pending,
    /// Repository has been successfully indexed.
    Indexed(FileTreeState),

    /// Repository indexing failed with the given error.
    Failed(RepoMetadataError),
}

/// Singleton model for managing local repository metadata.
///
/// This model tracks repositories on the local filesystem, using file watchers
/// to stay up to date and subscribing to `DetectedRepositories` for auto-indexing.
///
/// Consumers should access this through the [`RepoMetadataModel`](crate::wrapper_model::RepoMetadataModel)
/// wrapper rather than using this type directly.
pub struct LocalRepoMetadataModel {
    /// Mapping from repository path to its indexed state.
    repositories: HashMap<StandardizedPath, IndexedRepoState>,
    /// Refcounts for lazily-loaded standalone paths tracked in the model.
    lazy_loaded_paths: HashMap<StandardizedPath, usize>,
    /// 后台目录构建任务。键同时包含所属仓库与目标目录，避免不同仓库同路径冲突。
    build_tasks: HashMap<BuildTaskKey, BuildTask>,
    /// 每个仓库最多一个后台文件监视器更新任务。新事件会合并并替换旧任务，
    /// 防止旧扫描结果在新事件之后写回内存树。
    #[cfg(feature = "local_fs")]
    watcher_update_tasks: HashMap<StandardizedPath, WatcherUpdateTask>,
    /// File system watcher for monitoring changes.
    #[cfg(feature = "local_fs")]
    watcher: Option<ModelHandle<BulkFilesystemWatcher>>,
    /// When true, emit [`RepositoryMetadataEvent::IncrementalUpdateReady`]
    /// events after applying watcher mutations. Only the remote server
    /// variant enables this.
    emit_incremental_updates: bool,
}

#[derive(Debug, Clone, Default)]
struct RepoUpdate {
    added: HashSet<PathBuf>,
    deleted: HashSet<PathBuf>,
    moved: HashMap<PathBuf, PathBuf>,
}

impl RepoUpdate {
    /// 合并同一仓库尚未提交的 watcher 事件。
    ///
    /// 保留删除、创建和重命名的并集，再由后台读取最终文件系统状态判定最终
    /// 树形；这样即便前一轮扫描被替换，也不会遗漏已收到的事件。
    fn merge(&mut self, incoming: Self) {
        self.added.extend(incoming.added);
        self.deleted.extend(incoming.deleted);
        self.moved.extend(incoming.moved);
    }

    /// 规则文件或持有规则的目录发生变化时，需要重建规则快照和整棵树。
    fn requires_full_rescan(&self, gitignores: &[Gitignore]) -> bool {
        let directly_changes_gitignore = self
            .added
            .iter()
            .chain(&self.deleted)
            .chain(self.moved.keys())
            .chain(self.moved.values())
            .any(|path| path.file_name().is_some_and(|name| name == ".gitignore"));
        if directly_changes_gitignore {
            return true;
        }

        // 某些平台会将包含 `.gitignore` 的目录折叠为一次删除/重命名事件。
        // 若已缓存的规则目录位于被移除的子树内，保守地全量重建，避免继续
        // 对不存在的规则文件做匹配。
        self.deleted
            .iter()
            .chain(self.moved.values())
            .any(|changed_path| {
                gitignores.iter().any(|gitignore| {
                    !gitignore.path().as_os_str().is_empty()
                        && gitignore.path().starts_with(changed_path)
                })
            })
    }
}

/// 一个仓库当前唯一有效的 watcher 扫描任务。
#[cfg(feature = "local_fs")]
struct WatcherUpdateTask {
    /// 该任务覆盖的完整事件集合，用于新事件到来时无损合并。
    update: RepoUpdate,
    handle: SpawnedFutureHandle,
}

/// 后台扫描产生的结果。只有该结果会在 UI 线程写入内存树。
#[cfg(feature = "local_fs")]
enum WatcherTreeUpdate {
    Incremental {
        mutations: Vec<FileTreeMutation>,
        gitignores: Vec<Gitignore>,
    },
    FullRescan {
        root_entry: Entry,
        gitignores: Vec<Gitignore>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct BuildTaskKey {
    owner_repo_path: StandardizedPath,
    target_path: StandardizedPath,
}

impl BuildTaskKey {
    fn new(owner_repo_path: StandardizedPath, target_path: StandardizedPath) -> Self {
        Self {
            owner_repo_path,
            target_path,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BuildTaskKind {
    Index,
    DirectoryLoad,
}

struct BuildTask {
    kind: BuildTaskKind,
    handle: SpawnedFutureHandle,
    completion_waiters: Vec<oneshot::Sender<Result<(), String>>>,
}

/// Describes a single file-tree mutation computed on a background thread.
/// These are produced by `compute_file_tree_mutations` (filesystem I/O) and
/// consumed by `apply_file_tree_mutations` (tree-only, main thread).
#[derive(Debug)]
pub(crate) enum FileTreeMutation {
    /// Remove a path from the tree.
    Remove(PathBuf),
    /// Add a single file with pre-computed metadata.
    AddFile {
        path: PathBuf,
        is_ignored: bool,
        extension: Option<String>,
    },
    /// Add a directory with its fully-built subtree.
    AddDirectorySubtree { dir_path: PathBuf, subtree: Entry },
    /// Fallback: add a bare (unloaded) directory entry when `build_tree` fails.
    AddEmptyDirectory { path: PathBuf, is_ignored: bool },
}

/// A filter function for filtering repo contents during traversal.
type RepoContentFilter = dyn for<'a> Fn(&RepoContent<'a>) -> bool + Send + Sync;

pub struct GetContentsArgs {
    pub include_folders: bool,
    pub include_ignored: bool,
    /// Optional filter applied during traversal to skip entries early.
    /// Return `true` to include the entry, `false` to skip it.
    pub filter: Option<Arc<RepoContentFilter>>,
}

impl Default for GetContentsArgs {
    fn default() -> Self {
        Self {
            include_folders: true,
            include_ignored: false,
            filter: None,
        }
    }
}

impl GetContentsArgs {
    pub fn include_ignored(mut self) -> Self {
        self.include_ignored = true;
        self
    }

    pub fn exclude_folders(mut self) -> Self {
        self.include_folders = false;
        self
    }

    /// Sets a filter closure to be applied during traversal.
    /// Only entries for which the filter returns `true` will be included.
    pub fn with_filter<F>(self, filter: F) -> Self
    where
        F: for<'a> Fn(&RepoContent<'a>) -> bool + Send + Sync + 'static,
    {
        Self {
            include_folders: self.include_folders,
            include_ignored: self.include_ignored,
            filter: Some(Arc::new(filter)),
        }
    }
}

impl LocalRepoMetadataModel {
    /// Creates a new LocalRepoMetadataModel.
    #[cfg_attr(not(feature = "local_fs"), allow(unused_variables), allow(unused_mut))]
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        let mut model = Self {
            repositories: HashMap::new(),
            lazy_loaded_paths: HashMap::new(),
            build_tasks: HashMap::new(),
            #[cfg(feature = "local_fs")]
            watcher_update_tasks: HashMap::new(),
            #[cfg(feature = "local_fs")]
            watcher: None,
            emit_incremental_updates: false,
        };
        cfg_if::cfg_if! {
            if #[cfg(feature = "local_fs")] {
                let watcher = ctx.add_model(|ctx| {
                    BulkFilesystemWatcher::new(
                        std::time::Duration::from_secs(FILESYSTEM_WATCHER_DEBOUNCE_SECS),
                        ctx,
                    )
                });
                ctx.subscribe_to_model(&watcher, Self::handle_watcher_event);
                model.watcher = Some(watcher);

                ctx.subscribe_to_model(&DetectedRepositories::handle(ctx), |me, event, ctx| {
                    let DetectedRepositoriesEvent::DetectedGitRepo { repository, .. } = event;
                    let repo_path = repository.as_ref(ctx).root_dir().clone();
                    if let Err(e) = me.index_directory(repository.clone(), ctx) {
                        log::warn!(
                            "Failed to index directory {repo_path}: {e}"
                        );
                    }
                });
            }
        }

        model
    }

    /// Enables or disables emission of
    /// [`RepositoryMetadataEvent::IncrementalUpdateReady`] events after
    /// applying watcher mutations. Only the remote server variant should
    /// enable this.
    pub fn set_emit_incremental_updates(&mut self, enabled: bool) {
        self.emit_incremental_updates = enabled;
    }

    /// Handles events from the BulkFilesystemWatcher.
    #[cfg(feature = "local_fs")]
    fn handle_watcher_event(
        &mut self,
        event: &BulkFilesystemWatcherEvent,
        ctx: &mut ModelContext<Self>,
    ) {
        // UI 线程只做路径标准化、仓库路由和事件合并；绝不查询磁盘。
        let mut repo_updates: HashMap<StandardizedPath, RepoUpdate> = HashMap::new();

        for path in event.added_or_updated_iter() {
            if let Some(repo_path) = self.find_repository_for_watcher_path(path) {
                let repo_update = repo_updates.entry(repo_path).or_default();
                repo_update.added.insert(path.to_path_buf());
            }
        }

        for path in &event.deleted {
            if let Some(repo_path) = self.find_repository_for_watcher_path(path) {
                let repo_update = repo_updates.entry(repo_path).or_default();
                repo_update.deleted.insert(path.to_path_buf());
            }
        }

        for (to_path, from_path) in &event.moved {
            let to_repo_path = self.find_repository_for_watcher_path(to_path);
            let from_repo_path = self.find_repository_for_watcher_path(from_path);
            match (to_repo_path, from_repo_path) {
                (Some(to_repo_path), Some(from_repo_path)) if to_repo_path == from_repo_path => {
                    let repo_update = repo_updates.entry(to_repo_path).or_default();
                    repo_update
                        .moved
                        .insert(to_path.to_path_buf(), from_path.to_path_buf());
                }
                (Some(to_repo_path), Some(from_repo_path)) => {
                    repo_updates
                        .entry(to_repo_path)
                        .or_default()
                        .added
                        .insert(to_path.to_path_buf());
                    repo_updates
                        .entry(from_repo_path)
                        .or_default()
                        .deleted
                        .insert(from_path.to_path_buf());
                }
                (Some(to_repo_path), None) => {
                    repo_updates
                        .entry(to_repo_path)
                        .or_default()
                        .added
                        .insert(to_path.to_path_buf());
                }
                (None, Some(from_repo_path)) => {
                    repo_updates
                        .entry(from_repo_path)
                        .or_default()
                        .deleted
                        .insert(from_path.to_path_buf());
                }
                (None, None) => {}
            }
        }

        ctx.emit(RepositoryMetadataEvent::FileTreeUpdated {
            paths: repo_updates.keys().cloned().collect(),
        });
        for (repo_path, repo_scoped_update) in repo_updates {
            let repo_scoped_update =
                self.replace_watcher_update_task(&repo_path, repo_scoped_update);
            if let Some(IndexedRepoState::Indexed(state)) = self.repositories.get(&repo_path) {
                let repo_path_clone = repo_path.clone();
                let Some(local_repo_path) = repo_path.to_local_path() else {
                    continue;
                };
                let gitignores_clone = state.gitignores.clone();
                let lazy_load = self.lazy_loaded_paths.contains_key(&repo_path);
                let task_repo_path = repo_path.clone();
                let task_update = repo_scoped_update.clone();
                let task_future_id = Rc::new(Cell::new(None));
                let task_future_id_for_completion = task_future_id.clone();
                let update_handle = ctx.spawn(
                    async move {
                        let update = Self::compute_watcher_tree_update(
                            task_update,
                            local_repo_path,
                            gitignores_clone,
                            lazy_load,
                        )
                        .await;
                        (update, repo_path_clone, lazy_load)
                    },
                    move |model, (update, repo_path, lazy_load), ctx| {
                        if model
                            .finish_watcher_update_task(
                                &repo_path,
                                task_future_id_for_completion.get(),
                            )
                            .is_none()
                        {
                            return;
                        }
                        let Ok(update) = update else {
                            log::debug!(
                                "Failed to refresh repository tree after watcher event: {repo_path}"
                            );
                            return;
                        };
                        if let Some(IndexedRepoState::Indexed(state)) =
                            model.repositories.get_mut(&repo_path)
                        {
                            match update {
                                WatcherTreeUpdate::Incremental {
                                    mutations,
                                    gitignores,
                                } => {
                                    if let Some(repository) = state.repository_handle() {
                                        repository.update(ctx, |repository, _ctx| {
                                            repository.set_gitignores(gitignores.clone());
                                        });
                                    }
                                    state.gitignores = gitignores;
                                    let incremental_update = Self::apply_file_tree_mutations(
                                        &mut state.entry,
                                        mutations,
                                        lazy_load,
                                        model.emit_incremental_updates,
                                    );
                                    ctx.emit(RepositoryMetadataEvent::FileTreeEntryUpdated {
                                        path: repo_path,
                                    });

                                    if let Some(incremental_update) = incremental_update {
                                        ctx.emit(RepositoryMetadataEvent::IncrementalUpdateReady {
                                            update: incremental_update,
                                        });
                                    }
                                }
                                WatcherTreeUpdate::FullRescan {
                                    root_entry,
                                    gitignores,
                                } => {
                                    if let Some(repository) = state.repository_handle() {
                                        repository.update(ctx, |repository, _ctx| {
                                            repository.set_gitignores(gitignores.clone());
                                        });
                                    }
                                    state.entry = root_entry.into();
                                    state.gitignores = gitignores;
                                    // 远端会把 RepositoryUpdated 作为完整快照发送，避免
                                    // 将整棵重建树拆成大量增量补丁。
                                    ctx.emit(RepositoryMetadataEvent::RepositoryUpdated {
                                        path: repo_path.clone(),
                                    });
                                    ctx.emit(RepositoryMetadataEvent::FileTreeEntryUpdated {
                                        path: repo_path,
                                    });
                                }
                            }
                        }
                    },
                );
                task_future_id.set(Some(update_handle.future_id()));
                self.track_watcher_update_task(task_repo_path, repo_scoped_update, update_handle);
            }
        }
    }

    #[cfg(feature = "local_fs")]
    fn find_repository_for_standardized_path(
        &self,
        path: &StandardizedPath,
    ) -> Option<StandardizedPath> {
        self.repositories
            .iter()
            .filter(|(repo_path, state)| {
                path.starts_with(repo_path) && matches!(state, IndexedRepoState::Indexed(_))
            })
            .max_by_key(|(repo_path, _)| repo_path.as_str().len())
            .map(|(repo_path, _)| repo_path.clone())
    }

    #[cfg(feature = "local_fs")]
    fn find_repository_for_watcher_path(&self, path: &Path) -> Option<StandardizedPath> {
        let standardized_path = StandardizedPath::try_from_local(path).ok()?;
        self.find_repository_for_standardized_path(&standardized_path)
    }

    /// 兼容现有按真实文件系统路径查询仓库的调用方。
    ///
    /// watcher 的 UI 回调不得调用此方法；它使用
    /// [`find_repository_for_watcher_path`](Self::find_repository_for_watcher_path)
    /// 进行纯内存路径路由。
    #[cfg(feature = "local_fs")]
    pub fn find_repository_for_path(&self, path: &Path) -> Option<StandardizedPath> {
        let standardized_path = StandardizedPath::from_local_canonicalized(path).ok()?;
        self.find_repository_for_standardized_path(&standardized_path)
    }

    fn track_build_task(
        &mut self,
        key: BuildTaskKey,
        kind: BuildTaskKind,
        handle: SpawnedFutureHandle,
    ) {
        if let Some(existing_task) = self.build_tasks.insert(
            key,
            BuildTask {
                kind,
                handle,
                completion_waiters: Vec::new(),
            },
        ) {
            existing_task.handle.abort();
            Self::notify_completion_waiters(
                existing_task.completion_waiters,
                Err("项目树构建任务已被替换".to_string()),
            );
        }
    }

    fn finish_build_task(
        &mut self,
        key: &BuildTaskKey,
        future_id: Option<FutureId>,
    ) -> Option<BuildTask> {
        match (future_id, self.build_tasks.get(key)) {
            (Some(future_id), Some(task)) if task.handle.future_id() == future_id => {
                self.build_tasks.remove(key)
            }
            _ => None,
        }
    }

    fn subscribe_to_build_task(
        &mut self,
        key: &BuildTaskKey,
    ) -> Option<oneshot::Receiver<Result<(), String>>> {
        let task = self.build_tasks.get_mut(key)?;
        if task.kind != BuildTaskKind::DirectoryLoad {
            return None;
        }
        let (completion_tx, completion_rx) = oneshot::channel();
        task.completion_waiters.push(completion_tx);
        Some(completion_rx)
    }

    fn wait_for_build_task(
        completion_rx: oneshot::Receiver<Result<(), String>>,
    ) -> BoxFuture<'static, Result<(), RepoMetadataError>> {
        async move {
            completion_rx
                .await
                .unwrap_or_else(|_| Err("项目树构建任务已取消".to_string()))
                .map_err(RepoMetadataError::InvalidPath)
        }
        .boxed()
    }

    fn notify_completion_waiters(
        waiters: Vec<oneshot::Sender<Result<(), String>>>,
        result: Result<(), String>,
    ) {
        for waiter in waiters {
            let _ = waiter.send(result.clone());
        }
    }

    #[cfg(feature = "local_fs")]
    fn replace_watcher_update_task(
        &mut self,
        repo_path: &StandardizedPath,
        mut incoming_update: RepoUpdate,
    ) -> RepoUpdate {
        if let Some(existing_task) = self.watcher_update_tasks.remove(repo_path) {
            existing_task.handle.abort();
            incoming_update.merge(existing_task.update);
        }
        incoming_update
    }

    #[cfg(feature = "local_fs")]
    fn track_watcher_update_task(
        &mut self,
        repo_path: StandardizedPath,
        update: RepoUpdate,
        handle: SpawnedFutureHandle,
    ) {
        let existing_task = self
            .watcher_update_tasks
            .insert(repo_path, WatcherUpdateTask { update, handle });
        if let Some(existing_task) = existing_task {
            existing_task.handle.abort();
        }
    }

    #[cfg(feature = "local_fs")]
    fn finish_watcher_update_task(
        &mut self,
        repo_path: &StandardizedPath,
        future_id: Option<FutureId>,
    ) -> Option<WatcherUpdateTask> {
        let future_id = future_id?;
        match self.watcher_update_tasks.get(repo_path) {
            Some(task) if task.handle.future_id() == future_id => {
                self.watcher_update_tasks.remove(repo_path)
            }
            _ => None,
        }
    }

    #[cfg(feature = "local_fs")]
    fn abort_watcher_update_tasks_for_repo(&mut self, repo_path: &StandardizedPath) {
        if let Some(task) = self.watcher_update_tasks.remove(repo_path) {
            task.handle.abort();
        }
    }

    fn abort_builds_for_repo(&mut self, repo_path: &StandardizedPath) {
        let task_keys = self
            .build_tasks
            .keys()
            .filter(|key| &key.owner_repo_path == repo_path)
            .cloned()
            .collect::<Vec<_>>();
        for key in task_keys {
            if let Some(task) = self.build_tasks.remove(&key) {
                task.handle.abort();
                Self::notify_completion_waiters(
                    task.completion_waiters,
                    Err("项目树构建任务已取消".to_string()),
                );
            }
        }
        #[cfg(feature = "local_fs")]
        self.abort_watcher_update_tasks_for_repo(repo_path);
    }

    /// Adds or updates a repository's file tree state.
    fn add_repository_internal(
        &mut self,
        repo_path: StandardizedPath,
        state: FileTreeState,
        ctx: &mut ModelContext<Self>,
    ) -> Result<(), RepoMetadataError> {
        let local_path = repo_path
            .to_local_path()
            .ok_or_else(|| RepoMetadataError::PathEncodingMismatch(repo_path.clone()))?;

        // Register this path with the watcher if we have one. Skip the home
        // directory and its ancestors to avoid recursively watching unrelated
        // user data; those paths can still be listed in the file tree.
        #[cfg(feature = "local_fs")]
        {
            if let Some(ref watcher) = self.watcher {
                if !is_unsafe_watch_root(&local_path) {
                    let watch_path = local_path.clone();
                    watcher.update(ctx, |watcher, _ctx| {
                        use crate::entry::repo_watch_filter;
                        std::mem::drop(watcher.register_path(
                            &watch_path,
                            repo_watch_filter(watch_path.clone()),
                            RecursiveMode::Recursive,
                        ));
                    });
                }
            }
        }

        // Insert the repository state into the map
        let repo_path_for_event = repo_path.clone();
        self.repositories
            .insert(repo_path, IndexedRepoState::Indexed(state));

        ctx.emit(RepositoryMetadataEvent::RepositoryUpdated {
            path: repo_path_for_event,
        });

        Ok(())
    }

    /// Removes a repository from tracking.
    pub fn remove_repository(
        &mut self,
        repo_path: &StandardizedPath,
        ctx: &mut ModelContext<Self>,
    ) -> Result<(), RepoMetadataError> {
        self.abort_builds_for_repo(repo_path);
        if self.repositories.remove(repo_path).is_some() {
            // Unregister from watcher, mirroring the guard in add_repository_internal:
            // home directory and ancestors are never registered, so skip them here too.
            #[cfg(feature = "local_fs")]
            {
                if let Some(ref watcher) = self.watcher {
                    if let Some(local_path) = repo_path.to_local_path() {
                        if !is_unsafe_watch_root(&local_path) {
                            watcher.update(ctx, |watcher, _ctx| {
                                std::mem::drop(watcher.unregister_path(&local_path));
                            });
                        }
                    }
                }
            }

            ctx.emit(RepositoryMetadataEvent::RepositoryRemoved {
                path: repo_path.clone(),
            });

            Ok(())
        } else {
            Err(RepoMetadataError::RepoNotFound(repo_path.to_string()))
        }
    }

    pub fn get_repository(&self, repo_path: &StandardizedPath) -> Option<&FileTreeState> {
        match self.repositories.get(repo_path)? {
            IndexedRepoState::Indexed(state) => Some(state),
            IndexedRepoState::Pending => None,
            IndexedRepoState::Failed(_) => None,
        }
    }

    /// Returns the current [`IndexedRepoState`] for the specified repository or `None` if the
    /// repository is not being tracked.
    pub fn repository_state(&self, repo_path: &StandardizedPath) -> Option<&IndexedRepoState> {
        self.repositories.get(repo_path)
    }

    /// Checks if a repository is being tracked and indexed.
    pub fn has_repository(&self, repo_path: &StandardizedPath) -> bool {
        matches!(
            self.repositories.get(repo_path),
            Some(IndexedRepoState::Indexed(_))
        )
    }

    /// Returns whether the given path is tracked as a lazily-loaded standalone path.
    pub fn is_lazy_loaded_path(&self, path: &StandardizedPath) -> bool {
        self.lazy_loaded_paths.contains_key(path)
    }

    /// Lazily indexes a standalone path with only the first level of children.
    /// Registers the path with the file watcher for live updates.
    /// No-ops if the path is already tracked.
    #[cfg(feature = "local_fs")]
    pub fn index_lazy_loaded_path(
        &mut self,
        path: &StandardizedPath,
        ctx: &mut ModelContext<Self>,
    ) -> Result<(), RepoMetadataError> {
        // Already tracked as a lazy-loaded path — increase the refcount and keep the
        // existing watcher/model entry alive.
        if let Some(refcount) = self.lazy_loaded_paths.get_mut(path) {
            *refcount += 1;
            return Ok(());
        }

        // Already tracked as a real repo — don't overwrite it.
        if matches!(
            self.repositories.get(path),
            Some(IndexedRepoState::Indexed(_) | IndexedRepoState::Pending)
        ) {
            return Ok(());
        }

        let local_path = path
            .to_local_path()
            .ok_or_else(|| RepoMetadataError::PathEncodingMismatch(path.clone()))?;

        self.lazy_loaded_paths.insert(path.clone(), 1);
        self.repositories
            .insert(path.clone(), IndexedRepoState::Pending);

        let task_key = BuildTaskKey::new(path.clone(), path.clone());
        let task_key_for_completion = task_key.clone();
        let task_future_id = Rc::new(Cell::new(None));
        let task_future_id_for_completion = task_future_id.clone();
        let path_for_build = path.clone();
        let build_handle = ctx.spawn(
            async move {
                let result = Self::build_tree_from_local_path(
                    local_path,
                    1, // max_depth — only first level
                    IgnoredPathStrategy::Include,
                )
                .await;
                (result, path_for_build)
            },
            move |model, (build_result, path), ctx| {
                if model
                    .finish_build_task(
                        &task_key_for_completion,
                        task_future_id_for_completion.get(),
                    )
                    .is_none()
                {
                    return;
                }
                if !model.lazy_loaded_paths.contains_key(&path) {
                    return;
                }

                match build_result {
                    Ok((root_entry, _, gitignores)) => {
                        let state = FileTreeState::new_lazy_loaded(root_entry, gitignores);
                        if let Err(error) = model.add_repository_internal(path.clone(), state, ctx)
                        {
                            log::warn!("Failed to add lazy-loaded path {path}: {error:?}");
                            model.lazy_loaded_paths.remove(&path);
                            model
                                .repositories
                                .insert(path.clone(), IndexedRepoState::Failed(error));
                            ctx.emit(RepositoryMetadataEvent::UpdatingRepositoryFailed { path });
                        }
                    }
                    Err(error) => {
                        log::warn!("Failed to lazy-load path {path}: {error:?}");
                        model.lazy_loaded_paths.remove(&path);
                        model
                            .repositories
                            .insert(path.clone(), IndexedRepoState::Failed(error));
                        ctx.emit(RepositoryMetadataEvent::UpdatingRepositoryFailed { path });
                    }
                }
            },
        );
        task_future_id.set(Some(build_handle.future_id()));
        self.track_build_task(task_key, BuildTaskKind::Index, build_handle);
        Ok(())
    }

    /// Removes a lazily-loaded standalone path from tracking and unregisters the file watcher.
    #[cfg(feature = "local_fs")]
    pub fn remove_lazy_loaded_path(
        &mut self,
        path: &StandardizedPath,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some(refcount) = self.lazy_loaded_paths.get_mut(path) else {
            return;
        };
        if *refcount > 1 {
            *refcount -= 1;
            return;
        }
        self.lazy_loaded_paths.remove(path);
        // remove_repository unregisters the watcher and emits RepositoryRemoved.
        let _ = self.remove_repository(path, ctx);
    }

    /// Loads a specific directory inside an already-tracked tree.
    /// Emits `FileTreeEntryUpdated` so subscribers can sync.
    #[cfg(feature = "local_fs")]
    pub fn load_directory(
        &mut self,
        repo_root: &StandardizedPath,
        dir_path: &StandardizedPath,
        ctx: &mut ModelContext<Self>,
    ) -> Result<(), RepoMetadataError> {
        let completion = self.load_directory_with_completion(repo_root, dir_path, ctx)?;
        std::mem::drop(completion);
        Ok(())
    }

    /// 后台加载目录；相同目录的并发请求会复用同一个构建任务。
    #[cfg(feature = "local_fs")]
    pub fn load_directory_with_completion(
        &mut self,
        repo_root: &StandardizedPath,
        dir_path: &StandardizedPath,
        ctx: &mut ModelContext<Self>,
    ) -> Result<BoxFuture<'static, Result<(), RepoMetadataError>>, RepoMetadataError> {
        let task_key = BuildTaskKey::new(repo_root.clone(), dir_path.clone());
        if let Some(completion_rx) = self.subscribe_to_build_task(&task_key) {
            return Ok(Self::wait_for_build_task(completion_rx));
        }

        let Some(IndexedRepoState::Indexed(state)) = self.repositories.get_mut(repo_root) else {
            return Err(RepoMetadataError::RepoNotFound(repo_root.to_string()));
        };
        let Some(FileTreeEntryState::Directory(directory)) = state.entry.get(dir_path) else {
            return Err(RepoMetadataError::InvalidPath(format!(
                "Directory is not present in the project tree: {dir_path}"
            )));
        };
        if directory.loaded {
            return Ok(async { Ok(()) }.boxed());
        }

        let expected_directory_path = directory.path.clone();
        let mut gitignores = state.gitignores.clone();
        let dir_path_for_build = dir_path.to_local_path_lossy();
        let repo_root_for_completion = repo_root.clone();
        let dir_path_for_completion = dir_path.clone();
        let task_key_for_completion = task_key.clone();
        let task_future_id = Rc::new(Cell::new(None));
        let task_future_id_for_completion = task_future_id.clone();
        let (completion_tx, completion_rx) = oneshot::channel();
        let build_handle = ctx.spawn(
            async move {
                let _scan_permit = acquire_tree_scan_permit().await;
                let mut files = Vec::new();
                let mut file_limit = crate::entry::LAZY_LOAD_FILE_LIMIT;
                let entry = Entry::build_tree(
                    dir_path_for_build,
                    &mut files,
                    &mut gitignores,
                    Some(&mut file_limit),
                    1, // 只加载展开目录的下一层
                    0,
                    &IgnoredPathStrategy::Include,
                )
                .await?;
                Ok::<_, crate::entry::BuildTreeError>((entry, gitignores))
            },
            move |model, build_result, ctx| {
                let completion = if let Some(task) = model.finish_build_task(
                    &task_key_for_completion,
                    task_future_id_for_completion.get(),
                ) {
                    let completion = match build_result {
                        Ok((entry, gitignores)) => match model.repositories.get_mut(&repo_root_for_completion) {
                            Some(IndexedRepoState::Indexed(state)) => {
                                if !matches!(
                                    state.entry.get(&dir_path_for_completion),
                                    Some(FileTreeEntryState::Directory(directory))
                                        if !directory.loaded
                                            && Arc::ptr_eq(&directory.path, &expected_directory_path)
                                ) {
                                    Err(RepoMetadataError::InvalidPath(format!(
                                        "Directory changed while it was loading: {dir_path_for_completion}"
                                    )))
                                } else {
                                    let repository = state.repository_handle();
                                    state.entry.insert_entry_at_path(
                                        Arc::new(dir_path_for_completion.clone()),
                                        entry,
                                    );
                                    state.gitignores = gitignores.clone();
                                    if let Some(repository) = repository {
                                        repository.update(ctx, |repository, _| {
                                            repository.set_gitignores(gitignores);
                                        });
                                    }
                                    ctx.emit(RepositoryMetadataEvent::FileTreeEntryUpdated {
                                        path: repo_root_for_completion.clone(),
                                    });
                                    Ok(())
                                }
                            }
                            _ => Err(RepoMetadataError::RepoNotFound(
                                repo_root_for_completion.to_string(),
                            )),
                        },
                        Err(error) => {
                            log::warn!(
                                "Failed to load project-tree directory {dir_path_for_completion}: {error:?}"
                            );
                            Err(RepoMetadataError::BuildTree(error))
                        }
                    };
                    let waiter_completion = completion
                        .as_ref()
                        .map(|_| ())
                        .map_err(ToString::to_string);
                    Self::notify_completion_waiters(task.completion_waiters, waiter_completion);
                    completion
                } else {
                    Err(RepoMetadataError::InvalidPath(
                        "Project-tree directory load was replaced or cancelled".to_string(),
                    ))
                };
                let _ = completion_tx.send(completion);
            },
        );
        task_future_id.set(Some(build_handle.future_id()));
        self.track_build_task(task_key, BuildTaskKind::DirectoryLoad, build_handle);

        Ok(async move {
            completion_rx.await.unwrap_or_else(|_| {
                Err(RepoMetadataError::InvalidPath(
                    "Project-tree directory load was cancelled".to_string(),
                ))
            })
        }
        .boxed())
    }

    /// Checks whether the parent directory of `path` is loaded in the given entry.
    fn is_parent_loaded_in_entry(entry: &FileTreeEntry, path: &StandardizedPath) -> bool {
        let Some(parent) = path.parent() else {
            return true;
        };
        entry.get(&parent).is_some_and(|state| state.loaded())
    }

    /// 在后台验证根目录并构建树。所有实际文件系统访问都限制在此处。
    #[cfg(feature = "local_fs")]
    async fn build_tree_from_local_path(
        local_path: PathBuf,
        max_depth: usize,
        ignored_path_strategy: IgnoredPathStrategy,
    ) -> Result<(Entry, Vec<crate::entry::FileMetadata>, Vec<Gitignore>), RepoMetadataError> {
        let _scan_permit = acquire_tree_scan_permit().await;
        if !local_path.exists() {
            return Err(RepoMetadataError::RepoNotFound(
                local_path.to_string_lossy().into_owned(),
            ));
        }
        if !local_path.is_dir() {
            return Err(RepoMetadataError::InvalidPath(
                "Repository path must be a directory".to_string(),
            ));
        }

        let mut files = Vec::new();
        let mut gitignores = gitignores_for_directory(&local_path);
        let mut file_limit = MAX_FILES_PER_REPO;
        let entry = Entry::build_tree(
            local_path,
            &mut files,
            &mut gitignores,
            Some(&mut file_limit),
            max_depth,
            0,
            &ignored_path_strategy,
        )
        .await
        .map_err(RepoMetadataError::BuildTree)?;

        Ok((entry, files, gitignores))
    }

    /// 计算后台 watcher 更新。`.gitignore` 变更很少发生，但会影响整个仓库
    /// 的规则，因此使用一次完整后台重建保证结果正确。
    #[cfg(feature = "local_fs")]
    async fn compute_watcher_tree_update(
        update: RepoUpdate,
        local_repo_path: PathBuf,
        gitignores: Vec<Gitignore>,
        lazy_load: bool,
    ) -> Result<WatcherTreeUpdate, RepoMetadataError> {
        if update.requires_full_rescan(&gitignores) {
            let max_depth = if lazy_load { 1 } else { MAX_TREE_DEPTH };
            let ignored_path_strategy = if lazy_load {
                IgnoredPathStrategy::Include
            } else {
                IgnoredPathStrategy::IncludeLazy
            };
            let (root_entry, _, gitignores) =
                Self::build_tree_from_local_path(local_repo_path, max_depth, ignored_path_strategy)
                    .await?;
            return Ok(WatcherTreeUpdate::FullRescan {
                root_entry,
                gitignores,
            });
        }

        let (mutations, gitignores) =
            Self::compute_file_tree_mutations(update, &local_repo_path, gitignores).await;
        Ok(WatcherTreeUpdate::Incremental {
            mutations,
            gitignores,
        })
    }

    /// Phase 1: Computes file-tree mutations on a background thread.
    ///
    /// Performs all filesystem I/O (`exists()`, `is_dir()`, `build_tree()`,
    /// gitignore checks) and returns a lightweight list of mutations that can
    /// be applied to the tree on the main thread without cloning it.
    #[cfg(feature = "local_fs")]
    async fn compute_file_tree_mutations(
        update: RepoUpdate,
        repo_root: &Path,
        mut gitignores: Vec<Gitignore>,
    ) -> (Vec<FileTreeMutation>, Vec<Gitignore>) {
        let _scan_permit = acquire_tree_scan_permit().await;
        let mut mutations = Vec::new();
        let mut checked_gitignore_directories = HashSet::new();

        for changed_path in update
            .added
            .iter()
            .chain(&update.deleted)
            .chain(update.moved.keys())
            .chain(update.moved.values())
        {
            add_gitignores_for_path(
                repo_root,
                changed_path,
                changed_path.is_dir(),
                &mut gitignores,
                &mut checked_gitignore_directories,
            );
        }

        // Removals for deleted and moved-from paths
        for path_to_remove in update.deleted.iter().chain(update.moved.values()) {
            mutations.push(FileTreeMutation::Remove(path_to_remove.clone()));
        }

        // Additions for new and moved-to paths
        for path_to_add in update.added.iter().chain(update.moved.keys()) {
            if !path_to_add.exists() {
                continue;
            }

            let is_ignored = Self::path_is_ignored(path_to_add, &gitignores);

            if path_to_add.is_dir() {
                let mut files = Vec::new();
                let mut file_limit = MAX_FILES_PER_REPO;
                match Entry::build_tree(
                    path_to_add,
                    &mut files,
                    &mut gitignores,
                    Some(&mut file_limit),
                    MAX_TREE_DEPTH,
                    0,
                    &IgnoredPathStrategy::IncludeLazy,
                )
                .await
                {
                    Ok(subtree) => {
                        mutations.push(FileTreeMutation::AddDirectorySubtree {
                            dir_path: path_to_add.clone(),
                            subtree,
                        });
                    }
                    Err(e) => {
                        // Permission-denied / unreadable subdirs are expected when
                        // the watcher fires events for legitimate root paths (TCC-protected
                        // areas, symlinks, transient files); keep at debug to avoid noise.
                        log::debug!("Failed to build subtree for directory {path_to_add:?}: {e:?}");
                        mutations.push(FileTreeMutation::AddEmptyDirectory {
                            path: path_to_add.clone(),
                            is_ignored,
                        });
                    }
                }
            } else {
                let extension = path_to_add
                    .extension()
                    .and_then(|ext| ext.to_str().map(|s| s.to_owned()));
                mutations.push(FileTreeMutation::AddFile {
                    path: path_to_add.clone(),
                    is_ignored,
                    extension,
                });
            }
        }

        (mutations, gitignores)
    }

    /// Phase 2: Applies pre-computed mutations to the file tree on the main thread.
    ///
    /// No filesystem I/O — only tree-structure operations. When `lazy_load` is
    /// true, additions are skipped if the parent directory has not been expanded.
    ///
    /// When `emit_updates` is true,
    /// from the mutations that were actually applied (filtering out any skipped
    /// by `lazy_load`), suitable for sending to the remote client. When false,
    /// no update tracking is performed and the function returns `None`.
    pub(crate) fn apply_file_tree_mutations(
        root_entry: &mut FileTreeEntry,
        mutations: Vec<FileTreeMutation>,
        lazy_load: bool,
        emit_updates: bool,
    ) -> Option<RepoMetadataUpdate> {
        let emit = emit_updates;
        let mut remove_entries: Vec<StandardizedPath> = Vec::new();
        let mut update_entries: Vec<FileTreeEntryUpdate> = Vec::new();

        for mutation in mutations {
            match mutation {
                FileTreeMutation::Remove(ref path) => {
                    let Some(std_path) = StandardizedPath::try_from_local(path).ok() else {
                        continue;
                    };
                    root_entry.remove(&std_path);
                    if emit {
                        remove_entries.push(std_path);
                    }
                }
                FileTreeMutation::AddFile {
                    ref path,
                    is_ignored,
                    ref extension,
                } => {
                    let Some(std_path) = StandardizedPath::try_from_local(path).ok() else {
                        continue;
                    };
                    if lazy_load && !Self::is_parent_loaded_in_entry(root_entry, &std_path) {
                        continue;
                    }
                    let Some(parent) = std_path.parent() else {
                        continue;
                    };
                    Self::ensure_parent_directories_exist(root_entry, &parent);

                    let Some(parent_dir) = root_entry.find_parent_directory(&std_path) else {
                        continue;
                    };

                    // If the file already exists in the tree, just update its ignored flag
                    // to preserve the existing FileId.
                    if let Some(entry) = root_entry.get_mut(&std_path) {
                        entry.set_ignored(is_ignored);
                    } else {
                        let file_state = FileTreeEntryState::File(FileTreeFileMetadata {
                            path: Arc::new(std_path.clone()),
                            file_id: FileId::new(),
                            extension: extension.clone(),
                            ignored: is_ignored,
                        });
                        root_entry.insert_child_state(&parent_dir, file_state);
                    }
                    if emit {
                        update_entries.push(FileTreeEntryUpdate {
                            parent_path_to_replace: parent.clone(),
                            subtree_metadata: vec![RepoNodeMetadata::File(FileNodeMetadata {
                                path: std_path,
                                extension: extension.clone(),
                                ignored: is_ignored,
                            })],
                        });
                    }
                }
                FileTreeMutation::AddDirectorySubtree {
                    ref dir_path,
                    ref subtree,
                } => {
                    let Some(std_dir) = StandardizedPath::try_from_local(dir_path).ok() else {
                        continue;
                    };
                    if lazy_load && !Self::is_parent_loaded_in_entry(root_entry, &std_dir) {
                        continue;
                    }
                    if let Some(parent) = std_dir.parent() {
                        Self::ensure_parent_directories_exist(root_entry, &parent);
                    }
                    if let Some(parent_path) = root_entry.find_parent_directory(&std_dir) {
                        if let Some(FileTreeEntryState::Directory(directory)) =
                            root_entry.get_mut(&parent_path)
                        {
                            directory.loaded = true;
                        }
                        root_entry.remove(subtree.path());
                        root_entry.insert_entry_at_path(
                            Arc::new(subtree.path().clone()),
                            subtree.clone(),
                        );
                        if emit {
                            let parent_std = std_dir.parent().unwrap_or(std_dir.clone());
                            let metadata = flatten_entry_metadata(subtree);
                            update_entries.push(FileTreeEntryUpdate {
                                parent_path_to_replace: parent_std,
                                subtree_metadata: metadata,
                            });
                        }
                    }
                }
                FileTreeMutation::AddEmptyDirectory {
                    ref path,
                    is_ignored,
                } => {
                    let Some(std_path) = StandardizedPath::try_from_local(path).ok() else {
                        continue;
                    };
                    if lazy_load && !Self::is_parent_loaded_in_entry(root_entry, &std_path) {
                        continue;
                    }
                    let Some(parent) = std_path.parent() else {
                        continue;
                    };
                    Self::ensure_parent_directories_exist(root_entry, &parent);

                    let Some(parent_dir) = root_entry.find_parent_directory(&std_path) else {
                        continue;
                    };

                    let dir_state = FileTreeEntryState::Directory(FileTreeDirectoryEntryState {
                        path: Arc::new(std_path.clone()),
                        ignored: is_ignored,
                        loaded: false,
                    });
                    root_entry.insert_child_state(&parent_dir, dir_state);
                    if emit {
                        update_entries.push(FileTreeEntryUpdate {
                            parent_path_to_replace: parent.clone(),
                            subtree_metadata: vec![RepoNodeMetadata::Directory(
                                DirectoryNodeMetadata {
                                    path: std_path,
                                    ignored: is_ignored,
                                    loaded: false,
                                },
                            )],
                        });
                    }
                }
            }
        }

        if !emit {
            return None;
        }

        Some(RepoMetadataUpdate {
            repo_path: root_entry.root_directory().as_ref().clone(),
            remove_entries,
            update_entries,
        })
    }

    /// Delegates to [`FileTreeEntry::ensure_parent_directories_exist`].
    fn ensure_parent_directories_exist(
        root_entry: &mut FileTreeEntry,
        target_parent: &StandardizedPath,
    ) {
        root_entry.ensure_parent_directories_exist(target_parent);
    }

    /// Checks if a path matches any of the gitignore patterns
    #[cfg(feature = "local_fs")]
    fn path_is_ignored(path: &Path, gitignores: &[Gitignore]) -> bool {
        // Check if any component of the path is .git
        if path
            .components()
            .any(|component| component.as_os_str() == ".git")
        {
            return true;
        }

        // Check if path matches any gitignore patterns
        let is_dir = path.is_dir();
        matches_gitignores(path, is_dir, gitignores, true)
    }

    /// Indexes a repository from the given repository handle.
    #[cfg(feature = "local_fs")]
    pub fn index_directory(
        &mut self,
        repository: ModelHandle<Repository>,
        ctx: &mut ModelContext<'_, Self>,
    ) -> Result<(), RepoMetadataError> {
        let std_path = repository.as_ref(ctx).root_dir().clone();
        let local_path = std_path
            .to_local_path()
            .ok_or_else(|| RepoMetadataError::PathEncodingMismatch(std_path.clone()))?;

        let repo_path_str = std_path.to_string();

        // Check if the repository is already indexed or currently being indexed.
        // Allow re-indexing if the existing entry was a lazily-loaded path placeholder.
        match self.repositories.get(&std_path) {
            Some(IndexedRepoState::Indexed(_))
                if !self.lazy_loaded_paths.contains_key(&std_path) =>
            {
                log::debug!("Repository already indexed: {std_path}");
                return Ok(());
            }
            Some(IndexedRepoState::Indexed(_)) => {
                // Was a lazy-loaded path – allow upgrading to a real repo.
                log::info!("Upgrading lazy-loaded path to git repo: {repo_path_str}");
                self.lazy_loaded_paths.remove(&std_path);
                self.abort_builds_for_repo(&std_path);
            }
            Some(IndexedRepoState::Pending) if self.lazy_loaded_paths.contains_key(&std_path) => {
                log::info!("Replacing pending lazy-loaded path with git repo: {repo_path_str}");
                self.lazy_loaded_paths.remove(&std_path);
                self.abort_builds_for_repo(&std_path);
            }
            Some(IndexedRepoState::Pending) => {
                log::debug!("Repository already being indexed: {repo_path_str}");
                return Ok(());
            }
            Some(IndexedRepoState::Failed(error)) => {
                log::debug!(
                    "Repository indexing previously failed: {repo_path_str}, error: {error}"
                );
                log::info!("Retrying indexing for previously failed repository: {repo_path_str}");
                // Continue to retry indexing
            }
            None => {
                // Repository is not indexed and not pending, proceed with indexing
            }
        }

        // Mark the repository as pending to prevent duplicate work
        self.repositories
            .insert(std_path.clone(), IndexedRepoState::Pending);

        // Use the provided repository handle instead of creating a new one
        let repository_handle = repository;

        // Build the complete file tree for the repository asynchronously
        let repo_path_for_build = local_path;
        let repo_path_str_for_log = std_path.to_string();
        let std_path_for_completion = std_path;
        let repository_handle_for_completion = repository_handle.clone();
        let task_key = BuildTaskKey::new(
            std_path_for_completion.clone(),
            std_path_for_completion.clone(),
        );
        let task_key_for_completion = task_key.clone();
        let task_future_id = Rc::new(Cell::new(None));
        let task_future_id_for_completion = task_future_id.clone();

        let build_handle = ctx.spawn(
            async move {
                let build_result = Self::build_tree_from_local_path(
                    repo_path_for_build,
                    MAX_TREE_DEPTH,        // max_depth
                    IgnoredPathStrategy::IncludeLazy,
                )
                .await;
                (
                    build_result,
                    repo_path_str_for_log,
                    std_path_for_completion,
                    repository_handle_for_completion,
                )
            },
            move |model: &mut LocalRepoMetadataModel,
                  (
                      build_result,
                      repo_path_str,
                      std_repo_path,
                      repository_handle,
                   ),
                   ctx| {
                if model
                    .finish_build_task(
                        &task_key_for_completion,
                        task_future_id_for_completion.get(),
                    )
                    .is_none()
                {
                    return;
                }
                match build_result {
                    Ok((root_entry, files, gitignores_for_build)) => {
                        repository_handle.update(ctx, |repository, _ctx| {
                            repository.set_gitignores(gitignores_for_build.clone());
                        });
                        let state =
                            FileTreeState::new(root_entry, gitignores_for_build, Some(repository_handle));

                        if let Err(e) =
                            model.add_repository_internal(std_repo_path.clone(), state, ctx)
                        {
                            log::warn!("Failed to add repository {repo_path_str}: {e:?}");
                            // On failure, mark the repository as failed
                            model
                                .repositories
                                .insert(std_repo_path, IndexedRepoState::Failed(e));
                        } else {
                            log::info!(
                                "Successfully indexed repository: {} with {} files",
                                repo_path_str,
                                files.len()
                            );
                        }
                    }
                    Err(e) => {
                        safe_warn!(
                            safe: ("Failed to build file tree for repository: {e:?}"),
                            full: ("Failed to build file tree for repository {repo_path_str}: {e:?}")
                        );
                        send_telemetry_from_ctx!(RepoMetadataTelemetryEvent::BuildTreeFailed { error: format!("{e:#}") }, ctx);
                        ctx.emit(RepositoryMetadataEvent::UpdatingRepositoryFailed { path: std_repo_path.clone() });
                        model.repositories.insert(
                            std_repo_path,
                            IndexedRepoState::Failed(e),
                        );
                    }
                }
            },
        );
        task_future_id.set(Some(build_handle.future_id()));
        self.track_build_task(task_key, BuildTaskKind::Index, build_handle);

        Ok(())
    }

    /// Returns repository contents (files and optionally directories) in a given repository.
    pub fn get_repo_contents(
        &self,
        repo_path: &StandardizedPath,
        args: GetContentsArgs,
    ) -> Option<Vec<RepoContent<'_>>> {
        let state = match self.repositories.get(repo_path)? {
            IndexedRepoState::Indexed(state) => state,
            IndexedRepoState::Pending => return None,
            IndexedRepoState::Failed(_) => return None,
        };
        let mut contents = Vec::new();
        collect_contents_recursive(
            &state.entry,
            state.entry.root_directory(),
            &mut contents,
            &args,
        );
        Some(contents)
    }
}

impl warpui::Entity for LocalRepoMetadataModel {
    type Event = RepositoryMetadataEvent;
}

impl Drop for LocalRepoMetadataModel {
    fn drop(&mut self) {
        for task in self.build_tasks.drain().map(|(_, task)| task) {
            task.handle.abort();
        }
        #[cfg(feature = "local_fs")]
        for task in self.watcher_update_tasks.drain().map(|(_, task)| task) {
            task.handle.abort();
        }
    }
}

/// Helper function to recursively collect contents (files and optionally directories) from an Entry tree.
pub(crate) fn collect_contents_recursive<'a>(
    entry: &'a FileTreeEntry,
    current_path: &'a StandardizedPath,
    contents: &mut Vec<RepoContent<'a>>,
    args: &GetContentsArgs,
) {
    if !args.include_ignored && entry.ignored(current_path) {
        return;
    }

    match entry.get(current_path) {
        Some(FileTreeEntryState::File(metadata)) => {
            let content = RepoContent::File(metadata);
            if args.filter.as_ref().is_none_or(|f| f(&content)) {
                contents.push(content);
            }
        }
        Some(FileTreeEntryState::Directory(dir)) => {
            if args.include_folders {
                let content = RepoContent::Directory(dir);
                if args.filter.as_ref().is_none_or(|f| f(&content)) {
                    contents.push(content);
                }
            }

            for child in entry.child_paths(current_path) {
                collect_contents_recursive(entry, child, contents, args);
            }
        }
        None => {}
    }
}

// Test helpers
#[cfg(any(test, feature = "test-util"))]
impl LocalRepoMetadataModel {
    /// Insert a repository state directly for testing purposes.
    pub fn insert_test_state(&mut self, repo_path: StandardizedPath, state: FileTreeState) {
        self.repositories
            .insert(repo_path, IndexedRepoState::Indexed(state));
    }
}

#[cfg(test)]
#[path = "local_model_test.rs"]
mod tests;

#[cfg(all(test, feature = "local_fs"))]
mod is_unsafe_watch_root_tests {
    use super::is_unsafe_watch_root;
    use std::path::Path;

    #[test]
    fn rejects_home_and_its_ancestors() {
        let Some(home) = dirs::home_dir() else {
            // No $HOME (sandboxed CI etc.) — guard is a no-op there by design.
            return;
        };

        assert!(
            is_unsafe_watch_root(&home),
            "home directory itself ({}) must be rejected",
            home.display()
        );

        let filesystem_root = home.ancestors().last().unwrap_or(Path::new("/"));
        assert!(
            is_unsafe_watch_root(filesystem_root),
            "filesystem root must be rejected",
        );

        if let Some(parent) = home.parent() {
            assert!(
                is_unsafe_watch_root(parent),
                "home's parent ({}) must be rejected",
                parent.display(),
            );
        }
    }

    #[test]
    fn allows_directories_inside_home() {
        let Some(home) = dirs::home_dir() else {
            return;
        };

        let repo_inside_home = home.join("__zap_test_repo_path__");
        assert!(
            !is_unsafe_watch_root(&repo_inside_home),
            "{} (a directory inside home) must NOT be rejected",
            repo_inside_home.display(),
        );
    }

    #[test]
    fn allows_unrelated_paths() {
        let Some(home) = dirs::home_dir() else {
            return;
        };

        let tmp_path = std::env::temp_dir().join("__zap_test_unsafe_watch_root__");
        // Skip the case where tmp_path happens to be an ancestor of home
        // (vanishingly unlikely, but keeps the assertion meaningful).
        if !home.starts_with(&tmp_path) {
            assert!(
                !is_unsafe_watch_root(&tmp_path),
                "{} (unrelated tmp path) must NOT be rejected",
                tmp_path.display(),
            );
        }
    }
}
