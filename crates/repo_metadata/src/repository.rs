use std::collections::HashMap;
#[cfg(feature = "local_fs")]
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(feature = "local_fs")]
use std::path::{Path, PathBuf};

#[cfg(feature = "local_fs")]
use futures::channel::oneshot;
use futures::future::ready;
#[cfg(feature = "local_fs")]
use ignore::gitignore::Gitignore;
use warp_util::standardized_path::StandardizedPath;
use warpui::r#async::{BoxFuture, SpawnedFutureHandle};
#[cfg(feature = "local_fs")]
use warpui::SingletonEntity;
use warpui::{Entity, ModelContext, ModelHandle};

#[cfg(feature = "local_fs")]
use crate::watcher::DirectoryWatcher;
#[cfg(feature = "local_fs")]
use crate::{
    entry::{matches_gitignores, should_ignore_git_path},
    gitignores_for_directory,
};
use crate::{watcher::TaskQueue, RepoMetadataError, RepositoryUpdate};

/// Trait for entities that want to subscribe to repository file changes.
pub trait RepositorySubscriber: Send + Sync {
    /// Called when the subscriber is first added to build initial state.
    /// Returns a Future that completes when the scan is finished.
    fn on_scan(
        &mut self,
        repository: &Repository,
        ctx: &mut ModelContext<Repository>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

    /// Called when file changes are detected in the repository.
    /// Returns a Future that completes once updates are processed.
    fn on_files_updated(
        &mut self,
        repository: &Repository,
        update: &RepositoryUpdate,
        ctx: &mut ModelContext<Repository>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

    fn on_unsubscribe(&mut self, _ctx: &mut ModelContext<Repository>) {}
}

/// A unique identifier for repository subscribers.
pub type SubscriberId = usize;

pub struct StartWatching {
    pub subscriber_id: SubscriberId,
    pub registration_future: BoxFuture<'static, Result<(), RepoMetadataError>>,
}

/// Model for tracking a code repository that Zap is aware of.
pub struct Repository {
    /// The root directory of the repository.
    root_dir: StandardizedPath,
    /// External git directory path (e.g., for worktrees). This is the
    /// path to the **exact** per-worktree gitdir (e.g. `.git/worktrees/foo`).
    /// For the main worktree this is `None` (the gitdir is `root_dir/.git`).
    external_git_directory: Option<StandardizedPath>,
    /// The shared `.git` root directory that all worktrees of the same repo
    /// have in common. Derived from `external_git_directory` by walking up to
    /// the `.git` component. `None` when the repo is not a linked worktree.
    common_git_directory: Option<StandardizedPath>,
    /// Collection of subscribers interested in file changes.
    subscribers: HashMap<SubscriberId, Box<dyn RepositorySubscriber>>,
    /// Counter for generating unique subscriber IDs.
    next_subscriber_id: SubscriberId,
    /// Cached gitignore patterns for this repository.
    #[cfg(feature = "local_fs")]
    gitignores: Vec<Gitignore>,
    /// 已完成 Git ignore 探测的目录。即使目录没有 `.gitignore` 也会记录，避免
    /// 高频 watcher 事件重复访问磁盘。
    #[cfg(feature = "local_fs")]
    gitignore_checked_directories: HashSet<PathBuf>,
    /// Git ignore 快照版本。后台分类结果只能写回其开始时看到的版本，避免较早
    /// 的事件覆盖树扫描或较新的规则更新。
    #[cfg(feature = "local_fs")]
    gitignore_generation: u64,
    /// 使延迟完成的 watcher 初始化失效，避免最后一个订阅者已离开后仍注册监听。
    #[cfg(feature = "local_fs")]
    watch_generation: u64,

    task_queue: ModelHandle<TaskQueue>,
}

impl Repository {
    /// Creates a new Repository instance.
    pub(super) fn new(
        root_dir: StandardizedPath,
        external_git_directory: Option<StandardizedPath>,
        task_queue: ModelHandle<TaskQueue>,
    ) -> Self {
        #[cfg(feature = "local_fs")]
        let gitignores = Vec::new();
        #[cfg(feature = "local_fs")]
        let gitignore_checked_directories = HashSet::new();

        let common_git_directory = external_git_directory.as_ref().and_then(|ext| {
            ext.to_local_path()
                .and_then(|local| Self::derive_common_git_dir(&local))
                .and_then(|p| StandardizedPath::try_from_local(&p).ok())
                // Only store when it differs from external_git_directory.
                .filter(|common| common != ext)
        });

        Self {
            root_dir,
            external_git_directory,
            common_git_directory,
            subscribers: HashMap::new(),
            next_subscriber_id: 0,
            #[cfg(feature = "local_fs")]
            gitignores,
            #[cfg(feature = "local_fs")]
            gitignore_checked_directories,
            #[cfg(feature = "local_fs")]
            gitignore_generation: 0,
            #[cfg(feature = "local_fs")]
            watch_generation: 0,
            task_queue,
        }
    }

    /// Walk ancestors of the given path to find the `.git` component and return
    /// it as the shared git root. For example,
    /// `/repo/.git/worktrees/foo` → `/repo/.git`.
    fn derive_common_git_dir(external_git_dir: &std::path::Path) -> Option<std::path::PathBuf> {
        for ancestor in external_git_dir.ancestors() {
            if ancestor.file_name().and_then(|n| n.to_str()) == Some(".git") {
                return Some(ancestor.to_path_buf());
            }
        }
        None
    }

    /// The root directory of this repository.
    pub fn root_dir(&self) -> &StandardizedPath {
        &self.root_dir
    }

    /// The external git directory of this repository, if any.
    /// This is used for worktrees where the .git directory is external to the working tree.
    pub fn external_git_directory(&self) -> Option<&StandardizedPath> {
        self.external_git_directory.as_ref()
    }

    /// Returns the path to the actual `.git` directory for this repository.
    ///
    /// For normal repositories this is `root_dir/.git`. For worktrees, the
    /// `.git` entry in the working tree is a file (not a directory), so this
    /// returns the resolved `external_git_directory` instead.
    /// Subscribers should use this for per-worktree files like `index.lock`.
    pub fn git_dir(&self) -> std::path::PathBuf {
        self.external_git_directory
            .as_ref()
            .and_then(|d| d.to_local_path())
            .unwrap_or_else(|| self.root_dir.to_local_path_lossy().join(".git"))
    }

    /// Returns the shared `.git` root directory.
    ///
    /// For normal repos this is the same as `git_dir()`. For linked worktrees
    /// this is the common `.git` directory that all worktrees share (e.g.
    /// `/repo/.git`), distinct from the per-worktree gitdir.
    pub fn common_git_dir(&self) -> std::path::PathBuf {
        self.common_git_directory
            .as_ref()
            .and_then(|d| d.to_local_path())
            .unwrap_or_else(|| self.git_dir())
    }

    /// Returns the current watcher count.
    pub fn watcher_count(&self) -> usize {
        self.subscribers.len()
    }

    /// Starts watching this repository with the given subscriber.
    ///
    /// If this is the first subscriber, the repository root will be added to the
    /// RepositoryWatcher's set of watched paths.
    #[cfg_attr(not(feature = "local_fs"), allow(unused_variables))]
    pub fn start_watching(
        &mut self,
        subscriber: Box<dyn RepositorySubscriber>,
        ctx: &mut ModelContext<Self>,
    ) -> StartWatching {
        let subscriber_id = self.next_subscriber_id;
        self.next_subscriber_id += 1;

        // If this is the first subscriber, we need to start watching the repository
        #[cfg(feature = "local_fs")]
        let should_start_watching = self.subscribers.is_empty();

        self.subscribers.insert(subscriber_id, subscriber);

        #[cfg(feature = "local_fs")]
        let registration_future: BoxFuture<'static, Result<(), RepoMetadataError>> =
            if should_start_watching {
                // 这里只构造路径，不读取文件系统。Git ignore 规则在后台加载后
                // 才注册 watcher，避免在 UI 线程读取 `.gitignore`。
                let mut directories_to_watch = vec![self.root_dir.clone()];

                // Watch the per-worktree gitdir for worktree-specific events
                // (HEAD, index.lock under .git/worktrees/<name>/).
                if let Some(external_git_dir) = &self.external_git_directory {
                    directories_to_watch.push(external_git_dir.clone());
                }

                // For linked worktrees, also watch .git/refs so shared ref
                // changes (refs/heads/*) are visible even when the main
                // worktree isn't registered.
                if let Some(common_git_dir) = &self.common_git_directory {
                    if let Some(common_local) = common_git_dir.to_local_path() {
                        let refs_dir = common_local.join("refs").join("heads");
                        if let Ok(refs_std) = StandardizedPath::try_from_local(&refs_dir) {
                            directories_to_watch.push(refs_std);
                        }
                    }
                }

                let root_dir = self.root_dir.clone();
                self.watch_generation = self.watch_generation.wrapping_add(1);
                let watch_generation = self.watch_generation;
                let (registration_tx, registration_rx) = oneshot::channel();
                let initialization_handle = ctx.spawn(
                    async move {
                        let gitignores = root_dir
                            .to_local_path()
                            .map(|path| gitignores_for_directory(&path))
                            .unwrap_or_default();
                        (gitignores, directories_to_watch)
                    },
                    move |repository, (gitignores, directories_to_watch), ctx| {
                        if repository.watch_generation != watch_generation
                            || repository.subscribers.is_empty()
                        {
                            let _ = registration_tx.send(Err(RepoMetadataError::WatcherError(
                                anyhow::anyhow!("Repository watcher initialization was superseded"),
                            )));
                            return;
                        }
                        repository.set_gitignores(gitignores);
                        let registration =
                            DirectoryWatcher::handle(ctx).update(ctx, |watcher, ctx| {
                                watcher.start_watching_directories(directories_to_watch, ctx)
                            });
                        ctx.spawn(registration, move |repository, result, ctx| {
                            if repository.watch_generation != watch_generation
                                || repository.subscribers.is_empty()
                            {
                                let _ = registration_tx.send(Err(RepoMetadataError::WatcherError(
                                    anyhow::anyhow!(
                                        "Repository watcher registration was superseded"
                                    ),
                                )));
                                return;
                            }
                            // 只有后台 watcher 完成注册尝试后才做首次扫描，避免扫描与
                            // 监听之间的窗口遗漏修改。注册失败时仍保留首次扫描，以维持
                            // 原有的初始数据可用性；调用方会同时收到注册错误。
                            let subscriber_ids =
                                repository.subscribers.keys().copied().collect::<Vec<_>>();
                            for subscriber_id in subscriber_ids {
                                let repository_handle = ctx.handle();
                                repository.task_queue.update(ctx, |queue, ctx| {
                                    queue.enqueue_scan(repository_handle, subscriber_id, ctx);
                                });
                            }
                            let _ = registration_tx.send(result);
                        });
                    },
                );
                std::mem::drop(initialization_handle);
                Box::pin(async move {
                    registration_rx.await.unwrap_or_else(|_| {
                        Err(RepoMetadataError::WatcherError(anyhow::anyhow!(
                            "Repository watcher initialization was cancelled"
                        )))
                    })
                })
            } else {
                Box::pin(ready(Ok(())))
            };

        #[cfg(not(feature = "local_fs"))]
        let registration_future: BoxFuture<'static, Result<(), RepoMetadataError>> =
            Box::pin(async move { Ok(()) });

        #[cfg(feature = "local_fs")]
        if !should_start_watching {
            let self_handle = ctx.handle();
            self.task_queue.update(ctx, |queue, ctx| {
                queue.enqueue_scan(self_handle, subscriber_id, ctx);
            });
        }

        #[cfg(not(feature = "local_fs"))]
        {
            let self_handle = ctx.handle();
            self.task_queue.update(ctx, |queue, ctx| {
                queue.enqueue_scan(self_handle, subscriber_id, ctx);
            });
        }

        StartWatching {
            subscriber_id,
            registration_future,
        }
    }

    /// Stops watching this repository for the given subscriber.
    ///
    /// If this was the last subscriber, the repository root will be removed from the
    /// RepositoryWatcher's set of watched paths.
    #[cfg_attr(not(feature = "local_fs"), allow(unused_variables))]
    pub fn stop_watching(&mut self, subscriber_id: SubscriberId, ctx: &mut ModelContext<Self>) {
        let Some(mut subscriber) = self.subscribers.remove(&subscriber_id) else {
            return;
        };

        subscriber.on_unsubscribe(ctx);

        if self.subscribers.is_empty() {
            // If this was the last subscriber, notify the RepWatcher to stop watching.
            log::debug!(
                "All subscribers removed for {}, stopping watcher",
                self.root_dir
            );

            #[cfg(feature = "local_fs")]
            {
                self.watch_generation = self.watch_generation.wrapping_add(1);
                DirectoryWatcher::handle(ctx).update(ctx, |watcher, ctx| {
                    // Stop watching the working tree directory
                    std::mem::drop(watcher.stop_watching_directory(&self.root_dir, ctx));
                    // Mirror start_watching: stop per-worktree gitdir + shared refs.
                    if let Some(external_git_dir) = &self.external_git_directory {
                        std::mem::drop(watcher.stop_watching_directory(external_git_dir, ctx));
                    }
                    if let Some(common_git_dir) = &self.common_git_directory {
                        if let Some(common_local) = common_git_dir.to_local_path() {
                            let refs_dir = common_local.join("refs").join("heads");
                            if let Ok(refs_std) = StandardizedPath::try_from_local(&refs_dir) {
                                std::mem::drop(watcher.stop_watching_directory(&refs_std, ctx));
                            }
                        }
                    }
                });
            }
        }
    }

    /// Calls scan on a specific subscriber if it exists. Returns Some(Future) if the subscriber exists, None otherwise.
    pub(crate) fn scan_subscriber(
        &mut self,
        subscriber_id: SubscriberId,
        ctx: &mut ModelContext<Self>,
    ) -> Option<Pin<Box<dyn Future<Output = ()> + Send + 'static>>> {
        if let Some(mut subscriber) = self.subscribers.remove(&subscriber_id) {
            let future = subscriber.on_scan(self, ctx);
            self.subscribers.insert(subscriber_id, subscriber);
            Some(future)
        } else {
            None
        }
    }

    /// Notifies a specific subscriber about file changes.
    #[cfg(feature = "local_fs")]
    pub(crate) fn notify_subscriber(
        &mut self,
        subscriber_id: SubscriberId,
        update: &RepositoryUpdate,
        ctx: &mut ModelContext<Self>,
    ) -> Option<Pin<Box<dyn Future<Output = ()> + Send + 'static>>> {
        if let Some(mut subscriber) = self.subscribers.remove(&subscriber_id) {
            let future = subscriber.on_files_updated(self, update, ctx);
            self.subscribers.insert(subscriber_id, subscriber);
            Some(future)
        } else {
            None
        }
    }

    /// Returns the subscriber IDs for this repository.
    #[cfg(feature = "local_fs")]
    pub(crate) fn get_subscriber_ids(&self) -> Vec<SubscriberId> {
        self.subscribers.keys().cloned().collect()
    }

    /// Checks if a path is gitignored within this repository.
    #[cfg(feature = "local_fs")]
    pub fn check_gitignore_status(&self, path: &Path, is_dir: bool) -> bool {
        // Check if path is a .git internal file
        if should_ignore_git_path(path) {
            return true;
        }

        // Check if path matches gitignore patterns
        matches_gitignores(path, is_dir, &self.gitignores, true)
    }

    /// 用后台树扫描所得的完整规则快照更新通用仓库 watcher。
    #[cfg(feature = "local_fs")]
    pub(crate) fn set_gitignores(&mut self, gitignores: Vec<Gitignore>) {
        self.gitignores = gitignores;
        self.gitignore_checked_directories = self
            .gitignores
            .iter()
            .map(|gitignore| gitignore.path().to_path_buf())
            .filter(|path| !path.as_os_str().is_empty())
            .collect();
        if let Some(root_dir) = self.root_dir.to_local_path() {
            self.gitignore_checked_directories.insert(root_dir);
        }
        self.gitignore_generation = self.gitignore_generation.wrapping_add(1);
    }

    /// 返回后台 watcher 分类所需的完整 Git ignore 快照。
    #[cfg(feature = "local_fs")]
    pub(crate) fn gitignore_snapshot(&self) -> (Vec<Gitignore>, HashSet<PathBuf>, u64) {
        (
            self.gitignores.clone(),
            self.gitignore_checked_directories.clone(),
            self.gitignore_generation,
        )
    }

    /// 仅在快照仍是分类任务开始时的版本时写回结果。
    #[cfg(feature = "local_fs")]
    pub(crate) fn set_gitignore_snapshot_if_current(
        &mut self,
        expected_generation: u64,
        gitignores: Vec<Gitignore>,
        checked_directories: HashSet<PathBuf>,
    ) -> bool {
        if self.gitignore_generation != expected_generation {
            return false;
        }

        self.gitignores = gitignores;
        self.gitignore_checked_directories = checked_directories;
        self.gitignore_generation = self.gitignore_generation.wrapping_add(1);
        true
    }
}

impl Entity for Repository {
    type Event = ();
}

/// Coalescing merge for RepositoryUpdate with normalization rules.
fn merge_repository_updates(acc: &mut RepositoryUpdate, incoming: &RepositoryUpdate) {
    // 1) Moves first
    for (to, from) in &incoming.moved {
        if acc.added.remove(from) {
            acc.added.insert(to.clone());
            return;
        }
        if acc.modified.remove(from) {
            acc.modified.insert(to.clone());
            return;
        }

        // Collapse chain: if `from` was a prior destination, pull its original source
        let original_from = if let Some(prev_from) = acc.moved.remove(from) {
            prev_from
        } else {
            from.clone()
        };
        acc.moved.insert(to.clone(), original_from);
    }

    // 2) Adds next
    for p in &incoming.added {
        acc.deleted.remove(p);
        acc.moved.remove(p);
        acc.modified.remove(p);
        acc.added.insert(p.clone());
    }

    // 3) Modifies next
    for p in &incoming.modified {
        if acc.added.contains(p) {
            continue;
        }
        acc.deleted.remove(p);
        acc.moved.remove(p);
        acc.modified.insert(p.clone());
    }

    // 4) Deletes last
    for p in &incoming.deleted {
        // Added then removed within window => cancel
        if acc.added.remove(p) {
            continue;
        }

        acc.modified.remove(p);

        // Removing a move target => delete original source instead
        if let Some(from) = acc.moved.remove(p) {
            acc.deleted.insert(from);
            continue;
        }
        // Deleting the source of a recorded move is redundant; move already implies source removal
        let is_from_of_some_move = acc.moved.values().any(|f| f == p);
        if is_from_of_some_move {
            continue;
        }
        acc.deleted.insert(p.clone());
    }

    acc.commit_updated |= incoming.commit_updated;
    acc.index_lock_detected |= incoming.index_lock_detected;
}

/// A generic debouncing layer for any RepositorySubscriber.
pub struct BufferingRepositorySubscriber<S> {
    inner: Arc<Mutex<S>>,
    state: Arc<Mutex<BufferState>>,
    debounce: Duration,
}

#[derive(Default)]
struct BufferState {
    pending: RepositoryUpdate,
    /// Monotonic counter incremented for each incoming update; used to implement true debounce.
    version: u64,
    /// Whether the background flusher loop is currently running.
    flush_handle: Option<SpawnedFutureHandle>,
}

impl<S> BufferingRepositorySubscriber<S> {
    pub fn new(inner: S, debounce: Duration) -> Self {
        Self {
            inner: Arc::new(Mutex::new(inner)),
            state: Arc::new(Mutex::new(BufferState::default())),
            debounce,
        }
    }
}

impl<S> RepositorySubscriber for BufferingRepositorySubscriber<S>
where
    S: RepositorySubscriber + Send + Sync + 'static,
{
    fn on_scan(
        &mut self,
        repository: &Repository,
        ctx: &mut ModelContext<Repository>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        self.inner.lock().unwrap().on_scan(repository, ctx)
    }

    fn on_files_updated(
        &mut self,
        _repository: &Repository,
        update: &RepositoryUpdate,
        ctx: &mut ModelContext<Repository>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        {
            let mut st = self.state.lock().unwrap();
            merge_repository_updates(&mut st.pending, update);
            st.version = st.version.wrapping_add(1);

            // Start a single background flusher if it's not already running.
            if st.flush_handle.is_none() {
                let inner = Arc::clone(&self.inner);
                let state = Arc::clone(&self.state);
                let wait = self.debounce;

                st.flush_handle = Some(ctx.spawn(
                    async move {
                        // Loop until we observe a quiet period (version stable for `wait`).
                        loop {
                            // Capture current version, then wait.
                            let start_version = {
                                let st = state.lock().unwrap();
                                st.version
                            };
                            warpui::r#async::Timer::after(wait).await;

                            // If version unchanged, we're quiet; flush pending and exit loop.
                            let maybe_merged = {
                                // Yield before flushing to check if the current flush is cancelled.
                                futures_lite::future::yield_now().await;

                                let mut st = state.lock().unwrap();
                                if st.version == start_version {
                                    st.flush_handle = None;
                                    Some(std::mem::take(&mut st.pending))
                                } else {
                                    // Newer update arrived during the wait; try waiting again.
                                    None
                                }
                            };

                            if let Some(merged) = maybe_merged {
                                break (inner, merged);
                            }
                        }
                    },
                    |repo_model, (inner, merged), repo_ctx| {
                        if merged.is_empty() {
                            return;
                        }
                        if let Ok(mut inner) = inner.lock() {
                            let fut = inner.on_files_updated(repo_model, &merged, repo_ctx);
                            // Drive the subscriber's async update to completion.
                            repo_ctx.spawn(fut, |_, _, _| {});
                        }
                    },
                ));
            }
        }

        Box::pin(ready(()))
    }

    fn on_unsubscribe(&mut self, _ctx: &mut ModelContext<Repository>) {
        let Ok(mut st) = self.state.lock() else {
            return;
        };
        if let Some(handle) = st.flush_handle.take() {
            handle.abort();
        }
    }
}
