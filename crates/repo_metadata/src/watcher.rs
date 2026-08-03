use std::{
    collections::{hash_map::Entry, HashMap, HashSet, VecDeque},
    future::Future,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    pin::Pin,
};

#[cfg(feature = "local_fs")]
use futures::{future::OptionFuture, FutureExt as _};
#[cfg(feature = "local_fs")]
use ignore::gitignore::Gitignore;
use warpui::{Entity, ModelContext, ModelHandle, SingletonEntity, WeakModelHandle};

use warp_util::standardized_path::StandardizedPath;

use crate::{repository::SubscriberId, RepoMetadataError, Repository};

cfg_if::cfg_if! {
    if #[cfg(feature = "local_fs")] {
        use watcher::{BulkFilesystemWatcher, BulkFilesystemWatcherEvent};
        use crate::entry::{
            add_gitignores_for_path, extract_worktree_git_dir, gitignores_for_directory,
            invalidate_gitignores_under_path, is_commit_related_git_file, is_git_internal_path,
            is_index_lock_file, is_shared_git_ref, matches_gitignores,
        };
        /// Duration between filesystem watch events in milliseconds
        const FILESYSTEM_WATCHER_DEBOUNCE_MILLI_SECS: u64 = 500;
    }
}

const MAX_CONCURRENT_TASKS: usize = 2;

/// 后台 watcher 路由所需的纯内存快照。它在 UI 线程一次性复制，随后路径匹配、
/// Git ignore 分类和事件合并全部在后台执行。
#[cfg(feature = "local_fs")]
#[derive(Clone)]
struct RepositoryWatchRoute {
    root_dir: StandardizedPath,
    external_git_directory: Option<StandardizedPath>,
    common_git_directory: Option<StandardizedPath>,
    gitignores: Vec<Gitignore>,
    gitignore_checked_directories: HashSet<PathBuf>,
    gitignore_generation: u64,
    gitignore_snapshot_changed: bool,
}

/// 后台分类完成后写回 UI 线程的数据。规则快照与事件更新分开保存，避免仅为了
/// 缓存一次目录探测而触发订阅者更新。
#[cfg(feature = "local_fs")]
struct ClassifiedWatcherEvent {
    repo_updates: HashMap<StandardizedPath, RepositoryUpdate>,
    gitignore_snapshots: Vec<GitignoreSnapshotUpdate>,
}

#[cfg(feature = "local_fs")]
struct GitignoreSnapshotUpdate {
    root_dir: StandardizedPath,
    expected_generation: u64,
    gitignores: Vec<Gitignore>,
    checked_directories: HashSet<PathBuf>,
}

/// A global singleton model that records and watches directory changes.
/// It is important to note that the directory here doesn't equal to a git repository. To
/// reference a whether a path is a git repository or not, check `DetectedRepositories`.
pub struct DirectoryWatcher {
    /// Map of known directories to watch.
    directories: HashMap<StandardizedPath, ModelHandle<Repository>>,

    /// The filesystem watcher for monitoring changes.
    #[cfg(feature = "local_fs")]
    watcher: Option<ModelHandle<BulkFilesystemWatcher>>,

    /// 正在后台分类时后续到达的事件。串行分类保证 Git ignore 快照按 watcher
    /// 事件顺序演进，同时让 UI 回调只做 O(1) 入队。
    #[cfg(feature = "local_fs")]
    pending_watcher_events: VecDeque<BulkFilesystemWatcherEvent>,
    #[cfg(feature = "local_fs")]
    watcher_event_in_flight: bool,

    /// Handle to the internal processing queue model that orders scan & update tasks.
    processing_queue: ModelHandle<TaskQueue>,
}

impl DirectoryWatcher {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        cfg_if::cfg_if! {
            if #[cfg(feature = "local_fs")] {
                let fs_watcher = ctx.add_model(|ctx| {
                    BulkFilesystemWatcher::new(
                        std::time::Duration::from_millis(FILESYSTEM_WATCHER_DEBOUNCE_MILLI_SECS),
                        ctx,
                    )
                });
                ctx.subscribe_to_model(&fs_watcher, Self::handle_watcher_event);
            } else {
                // Silence an unused parameter warning.
                let _ = ctx;
            }
        }

        let processing_queue = ctx.add_model(TaskQueue::new);
        ctx.subscribe_to_model(&processing_queue, Self::handle_queue_event);

        Self {
            directories: Default::default(),
            #[cfg(feature = "local_fs")]
            watcher: Some(fs_watcher),
            #[cfg(feature = "local_fs")]
            pending_watcher_events: VecDeque::new(),
            #[cfg(feature = "local_fs")]
            watcher_event_in_flight: false,
            processing_queue,
        }
    }

    /// Test-only constructor that uses a stub filesystem watcher with no background thread,
    /// preventing thread leaks in tests.
    #[cfg(any(test, feature = "test-util"))]
    pub fn new_for_testing(ctx: &mut ModelContext<Self>) -> Self {
        cfg_if::cfg_if! {
            if #[cfg(feature = "local_fs")] {
                let fs_watcher = ctx.add_model(|_ctx| BulkFilesystemWatcher::new_for_test());
                ctx.subscribe_to_model(&fs_watcher, Self::handle_watcher_event);
            } else {
                let _ = ctx;
            }
        }

        let processing_queue = ctx.add_model(TaskQueue::new);
        ctx.subscribe_to_model(&processing_queue, Self::handle_queue_event);

        Self {
            directories: Default::default(),
            #[cfg(feature = "local_fs")]
            watcher: Some(fs_watcher),
            #[cfg(feature = "local_fs")]
            pending_watcher_events: VecDeque::new(),
            #[cfg(feature = "local_fs")]
            watcher_event_in_flight: false,
            processing_queue,
        }
    }

    /// Given a path, return the watched directory that contains it.
    pub fn get_watched_directory_for_path(&self, path: &Path) -> Option<ModelHandle<Repository>> {
        let standardized = StandardizedPath::try_from_local(path).ok()?;
        self.find_containing_directory(&standardized)
    }

    /// Find the watched directory that contains the given path, if any.
    fn find_containing_directory(
        &self,
        path: &StandardizedPath,
    ) -> Option<ModelHandle<Repository>> {
        let mut current = Some(path.clone());
        while let Some(ancestor) = current {
            if let Some(repo) = self.directories.get(&ancestor) {
                return Some(repo.clone());
            }
            current = ancestor.parent();
        }
        None
    }

    /// Check if a directory is registered for the given path.
    pub fn is_directory_watched(&self, path: &StandardizedPath) -> bool {
        self.directories.contains_key(path)
    }

    /// 将当前仓库状态提取为后台 watcher 可以安全使用的纯内存快照。
    #[cfg(feature = "local_fs")]
    fn watcher_routes(&self, ctx: &ModelContext<Self>) -> Vec<RepositoryWatchRoute> {
        self.directories
            .iter()
            .map(|(root_dir, repository)| {
                let repository = repository.as_ref(ctx);
                let (gitignores, gitignore_checked_directories, gitignore_generation) =
                    repository.gitignore_snapshot();
                let common_git_directory =
                    StandardizedPath::try_from_local(repository.common_git_dir().as_path()).ok();
                RepositoryWatchRoute {
                    root_dir: root_dir.clone(),
                    external_git_directory: repository.external_git_directory().cloned(),
                    common_git_directory,
                    gitignores,
                    gitignore_checked_directories,
                    gitignore_generation,
                    gitignore_snapshot_changed: false,
                }
            })
            .collect()
    }

    /// Register a known code directory. If the directory already exists, it will not be re-registered.
    pub fn add_directory(
        &mut self,
        repository_path: StandardizedPath,
        ctx: &mut ModelContext<Self>,
    ) -> Result<ModelHandle<Repository>, RepoMetadataError> {
        self.add_directory_with_git_dir(repository_path, None, ctx)
    }

    /// Register a known code directory with optional external git directory.
    /// If the directory already exists, it will not be re-registered.
    pub fn add_directory_with_git_dir(
        &mut self,
        repository_path: StandardizedPath,
        external_git_directory: Option<StandardizedPath>,
        ctx: &mut ModelContext<Self>,
    ) -> Result<ModelHandle<Repository>, RepoMetadataError> {
        if repository_path.to_local_path().is_none() {
            return Err(RepoMetadataError::PathEncodingMismatch(repository_path));
        }

        // Check if there's an existing registration to reuse.
        let entry = self.directories.entry(repository_path);
        if let Entry::Occupied(ref entry) = entry {
            log::debug!("Using already-registered repository");
            return Ok(entry.get().clone());
        }

        // The repository is either not registered, or has expired.
        let queue_handle = self.processing_queue.clone();
        let repository_handle = ctx.add_model(|_ctx| {
            Repository::new(
                entry.key().clone(),
                external_git_directory.clone(),
                queue_handle,
            )
        });
        entry.insert_entry(repository_handle.clone());

        Ok(repository_handle)
    }

    /// Starts watching multiple directories for filesystem changes.
    ///
    /// The returned future resolves once all directories are registered.
    #[cfg(feature = "local_fs")]
    pub(crate) fn start_watching_directories(
        &mut self,
        directory_paths: Vec<StandardizedPath>,
        ctx: &mut ModelContext<Self>,
    ) -> impl Future<Output = Result<(), RepoMetadataError>> {
        let futures: Vec<_> = directory_paths
            .into_iter()
            .map(|path| self.start_watching_directory(&path, ctx))
            .collect();

        async move {
            for future in futures {
                future.await?;
            }
            Ok(())
        }
    }
    /// Starts watching a directory for filesystem changes.
    ///
    /// The returned future resolves once the directory is registered. Filesystem changes before
    /// this may not be observed.
    #[cfg(feature = "local_fs")]
    pub(crate) fn start_watching_directory(
        &mut self,
        directory_path: &StandardizedPath,
        ctx: &mut ModelContext<Self>,
    ) -> impl Future<Output = Result<(), RepoMetadataError>> {
        let local_path = directory_path.to_local_path();
        let registration_future = if let Some(ref watcher) = self.watcher {
            if let Some(local_path) = local_path.clone() {
                watcher.update(ctx, |watcher, _ctx| {
                    use crate::entry::repo_watch_filter;
                    use notify_debouncer_full::notify::RecursiveMode;

                    Some(watcher.register_path(
                        &local_path,
                        repo_watch_filter(local_path.clone()),
                        RecursiveMode::Recursive,
                    ))
                })
            } else {
                log::warn!("Cannot watch non-local path: {directory_path}");
                None
            }
        } else {
            log::warn!("No watcher available");
            None
        };

        let path_display = directory_path.to_string();
        OptionFuture::from(registration_future).map(move |result| match result {
            Some(Ok(())) => {
                log::debug!("Started watching {path_display}");
                Ok(())
            }
            Some(Err(e)) => {
                log::debug!("Failed to start watching {path_display}: {e:#}");
                Err(e.into())
            }
            None => Ok(()),
        })
    }

    /// Stops watching a directory for filesystem changes.
    #[cfg(feature = "local_fs")]
    pub(crate) fn stop_watching_directory(
        &mut self,
        directory_path: &StandardizedPath,
        ctx: &mut ModelContext<Self>,
    ) -> impl Future<Output = Result<(), anyhow::Error>> {
        cfg_if::cfg_if! {
            if #[cfg(feature = "local_fs")] {
                let local_path = directory_path.to_local_path();
                let unregistration_future = if let Some(ref watcher) = self.watcher {
                    if let Some(local_path) = local_path {
                        watcher.update(ctx, |watcher, _ctx| {
                            Some(watcher.unregister_path(&local_path))
                        })
                    } else {
                        log::warn!("Cannot unwatch non-local path: {directory_path}");
                        None
                    }
                } else {
                    log::warn!("No watcher available");
                    None
                };

                let path_display = directory_path.to_string();
                OptionFuture::from(unregistration_future).map(move |result| match result {
                    Some(Ok(())) => {
                        log::debug!("Stopped watching {path_display}");
                        Ok(())
                    }
                    Some(Err(e)) => {
                        log::warn!("Failed to stop watching {path_display}: {e:#}");
                        Err(e)
                    }
                    None => Ok(()),
                })
            } else {
                async { Ok(()) }
            }
        }
    }

    /// Handles events from the internal task queue.
    fn handle_queue_event(&mut self, event: &TaskQueueEvent, ctx: &mut ModelContext<Self>) {
        let &TaskQueueEvent::TaskEnqueued = event;
        self.processing_queue.update(ctx, |queue, ctx| {
            queue.advance(ctx);
        });
    }

    /// Handles filesystem watcher events.
    #[cfg(feature = "local_fs")]
    pub(crate) fn handle_watcher_event(
        &mut self,
        event: &BulkFilesystemWatcherEvent,
        ctx: &mut ModelContext<Self>,
    ) {
        // UI 回调只做常数时间入队；路径归类、Git ignore 匹配和规则文件访问都
        // 由串行后台任务完成，避免生成目录变化拖慢渲染或乱序覆盖规则快照。
        self.pending_watcher_events.push_back(event.clone());
        self.start_next_watcher_event(ctx);
    }

    #[cfg(feature = "local_fs")]
    fn start_next_watcher_event(&mut self, ctx: &mut ModelContext<Self>) {
        if self.watcher_event_in_flight {
            return;
        }

        let Some(event) = self.pending_watcher_events.pop_front() else {
            return;
        };
        let routes = self.watcher_routes(ctx);
        if routes.is_empty() {
            // 没有订阅仓库时积压事件没有消费者，直接丢弃余量。
            self.pending_watcher_events.clear();
            return;
        }

        self.watcher_event_in_flight = true;
        ctx.spawn(
            async move { classify_watcher_event(event, routes) },
            |watcher, classified_event, ctx| {
                watcher.watcher_event_in_flight = false;
                watcher.enqueue_watcher_updates(classified_event, ctx);
                watcher.start_next_watcher_event(ctx);
            },
        );
    }

    #[cfg(feature = "local_fs")]
    fn enqueue_watcher_updates(
        &mut self,
        classified_event: ClassifiedWatcherEvent,
        ctx: &mut ModelContext<Self>,
    ) {
        for snapshot in classified_event.gitignore_snapshots {
            let Some(repository) = self.directories.get(&snapshot.root_dir) else {
                continue;
            };
            repository.update(ctx, |repository, _| {
                repository.set_gitignore_snapshot_if_current(
                    snapshot.expected_generation,
                    snapshot.gitignores,
                    snapshot.checked_directories,
                );
            });
        }

        self.processing_queue.update(ctx, |queue, ctx| {
            for (repo_path, repo_update) in classified_event.repo_updates {
                let Some(repo_handle) = self.directories.get(&repo_path) else {
                    continue;
                };
                let subscriber_ids = repo_handle.read(ctx, |repo, _| repo.get_subscriber_ids());
                for subscriber_id in subscriber_ids {
                    queue.enqueue_incremental_update(
                        repo_handle.downgrade(),
                        subscriber_id,
                        repo_update.clone(),
                        ctx,
                    );
                }
            }
        });
    }
}

#[cfg(feature = "local_fs")]
#[derive(Clone, Copy)]
enum PathUpdateKind {
    Added,
    Modified,
    Deleted,
}

/// 在后台按仓库路由一个已去抖的文件系统事件。
///
/// 此函数只接收值类型的快照，不访问 `ModelHandle` 或 UI 上下文；目录上的
/// `.gitignore` 探测也在这里完成，因此慢磁盘、网络盘和安全软件不会阻塞渲染线程。
#[cfg(feature = "local_fs")]
fn classify_watcher_event(
    event: BulkFilesystemWatcherEvent,
    mut routes: Vec<RepositoryWatchRoute>,
) -> ClassifiedWatcherEvent {
    let mut repo_updates = HashMap::new();

    // 同一批去抖事件里，规则文件和普通文件的迭代顺序并不稳定。先使所有受影响
    // 仓库的规则快照失效，确保后续普通路径永远按磁盘上的最新 `.gitignore`
    // 分类，而不是偶发地沿用旧 matcher。
    for path in event
        .added
        .iter()
        .chain(&event.modified)
        .chain(&event.deleted)
        .chain(event.moved.keys())
        .chain(event.moved.values())
    {
        if path.file_name().is_some_and(|name| name == ".gitignore") {
            if let Some(route_index) = route_index_for_path(path, &routes) {
                reset_route_gitignores(&mut routes[route_index]);
            }
        }
    }

    for path in &event.added {
        if is_git_internal_path(path) {
            classify_git_event(path, &routes, &mut repo_updates);
        } else {
            classify_regular_path(
                path,
                event.is_directory(path),
                false,
                PathUpdateKind::Added,
                &mut routes,
                &mut repo_updates,
            );
        }
    }

    for path in &event.modified {
        if is_git_internal_path(path) {
            classify_git_event(path, &routes, &mut repo_updates);
        } else {
            classify_regular_path(
                path,
                event.is_directory(path),
                false,
                PathUpdateKind::Modified,
                &mut routes,
                &mut repo_updates,
            );
        }
    }

    for path in &event.deleted {
        if is_git_internal_path(path) {
            classify_git_event(path, &routes, &mut repo_updates);
        } else {
            classify_regular_path(
                path,
                false,
                true,
                PathUpdateKind::Deleted,
                &mut routes,
                &mut repo_updates,
            );
        }
    }

    for (to_path, from_path) in &event.moved {
        if is_git_internal_path(to_path) || is_git_internal_path(from_path) {
            classify_git_event(to_path, &routes, &mut repo_updates);
            classify_git_event(from_path, &routes, &mut repo_updates);
            continue;
        }

        let to_route = route_index_for_path(to_path, &routes);
        let from_route = route_index_for_path(from_path, &routes);
        let is_directory = event.is_directory(to_path);
        match (to_route, from_route) {
            (Some(to_index), Some(from_index)) if to_index == from_index => {
                let route = &mut routes[to_index];
                prepare_route_gitignores(route, from_path, is_directory, true);
                prepare_route_gitignores(route, to_path, is_directory, false);
                let to_target = target_file(route, to_path, is_directory);
                let from_target = target_file(route, from_path, is_directory);
                repo_updates
                    .entry(route.root_dir.clone())
                    .or_default()
                    .moved
                    .insert(to_target, from_target);
            }
            (Some(_), Some(_)) => {
                // 跨仓库移动不能作为单个 `moved` 事件交给任一订阅者；分别通知
                // 源仓库删除和目标仓库新增，避免文件树残留。
                classify_regular_path(
                    from_path,
                    is_directory,
                    true,
                    PathUpdateKind::Deleted,
                    &mut routes,
                    &mut repo_updates,
                );
                classify_regular_path(
                    to_path,
                    is_directory,
                    false,
                    PathUpdateKind::Added,
                    &mut routes,
                    &mut repo_updates,
                );
            }
            (Some(_), None) => classify_regular_path(
                to_path,
                is_directory,
                false,
                PathUpdateKind::Added,
                &mut routes,
                &mut repo_updates,
            ),
            (None, Some(_)) => classify_regular_path(
                from_path,
                is_directory,
                true,
                PathUpdateKind::Deleted,
                &mut routes,
                &mut repo_updates,
            ),
            (None, None) => {}
        }
    }

    let gitignore_snapshots = routes
        .into_iter()
        .filter(|route| route.gitignore_snapshot_changed)
        .map(|route| GitignoreSnapshotUpdate {
            root_dir: route.root_dir,
            expected_generation: route.gitignore_generation,
            gitignores: route.gitignores,
            checked_directories: route.gitignore_checked_directories,
        })
        .collect();

    ClassifiedWatcherEvent {
        repo_updates,
        gitignore_snapshots,
    }
}

#[cfg(feature = "local_fs")]
fn classify_regular_path(
    path: &Path,
    is_directory: bool,
    was_removed: bool,
    kind: PathUpdateKind,
    routes: &mut [RepositoryWatchRoute],
    repo_updates: &mut HashMap<StandardizedPath, RepositoryUpdate>,
) {
    let Some(route_index) = route_index_for_path(path, routes) else {
        return;
    };
    let route = &mut routes[route_index];
    prepare_route_gitignores(route, path, is_directory, was_removed);
    let target = target_file(route, path, is_directory);
    let update = repo_updates.entry(route.root_dir.clone()).or_default();
    match kind {
        PathUpdateKind::Added => {
            update.added.insert(target);
        }
        PathUpdateKind::Modified => {
            if !update.added.contains(&target) {
                update.modified.insert(target);
            }
        }
        PathUpdateKind::Deleted => {
            update.deleted.insert(target);
        }
    }
}

#[cfg(feature = "local_fs")]
fn classify_git_event(
    path: &Path,
    routes: &[RepositoryWatchRoute],
    repo_updates: &mut HashMap<StandardizedPath, RepositoryUpdate>,
) {
    let commit_updated = is_commit_related_git_file(path);
    let index_lock_detected = is_index_lock_file(path);
    if !commit_updated && !index_lock_detected {
        return;
    }

    for route_index in route_indices_for_git_event(path, routes) {
        let route = &routes[route_index];
        let update = repo_updates.entry(route.root_dir.clone()).or_default();
        update.commit_updated |= commit_updated;
        update.index_lock_detected |= index_lock_detected;
    }
}

#[cfg(feature = "local_fs")]
fn route_indices_for_git_event(path: &Path, routes: &[RepositoryWatchRoute]) -> Vec<usize> {
    if let Some(worktree_git_dir) = extract_worktree_git_dir(path) {
        let Ok(worktree_git_dir) = StandardizedPath::try_from_local(&worktree_git_dir) else {
            return Vec::new();
        };
        return routes
            .iter()
            .enumerate()
            .filter_map(|(index, route)| {
                (route.external_git_directory.as_ref() == Some(&worktree_git_dir)).then_some(index)
            })
            .collect();
    }

    if is_shared_git_ref(path) {
        let Ok(git_path) = StandardizedPath::try_from_local(path) else {
            return Vec::new();
        };
        return routes
            .iter()
            .enumerate()
            .filter_map(|(index, route)| {
                (git_path.starts_with(&route.root_dir)
                    || route
                        .common_git_directory
                        .as_ref()
                        .is_some_and(|common_dir| git_path.starts_with(common_dir)))
                .then_some(index)
            })
            .collect();
    }

    route_index_for_path(path, routes).into_iter().collect()
}

#[cfg(feature = "local_fs")]
fn route_index_for_path(path: &Path, routes: &[RepositoryWatchRoute]) -> Option<usize> {
    let path = StandardizedPath::try_from_local(path).ok()?;
    routes
        .iter()
        .enumerate()
        .filter(|(_, route)| path.starts_with(&route.root_dir))
        .max_by_key(|(_, route)| route.root_dir.as_str().len())
        .map(|(index, _)| index)
}

#[cfg(feature = "local_fs")]
fn target_file(route: &RepositoryWatchRoute, path: &Path, is_directory: bool) -> TargetFile {
    TargetFile::new(
        path.to_path_buf(),
        matches_gitignores(path, is_directory, &route.gitignores, true),
    )
}

#[cfg(feature = "local_fs")]
fn prepare_route_gitignores(
    route: &mut RepositoryWatchRoute,
    path: &Path,
    is_directory: bool,
    was_removed: bool,
) {
    if path.file_name().is_some_and(|name| name == ".gitignore") {
        reset_route_gitignores(route);
    } else if was_removed {
        let before = (
            route.gitignores.len(),
            route.gitignore_checked_directories.len(),
        );
        invalidate_gitignores_under_path(
            path,
            &mut route.gitignores,
            &mut route.gitignore_checked_directories,
        );
        if before
            != (
                route.gitignores.len(),
                route.gitignore_checked_directories.len(),
            )
        {
            route.gitignore_snapshot_changed = true;
        }
    }

    let Some(root_dir) = route.root_dir.to_local_path() else {
        return;
    };
    let before = (
        route.gitignores.len(),
        route.gitignore_checked_directories.len(),
    );
    add_gitignores_for_path(
        &root_dir,
        path,
        is_directory,
        &mut route.gitignores,
        &mut route.gitignore_checked_directories,
    );
    if before
        != (
            route.gitignores.len(),
            route.gitignore_checked_directories.len(),
        )
    {
        route.gitignore_snapshot_changed = true;
    }
}

#[cfg(feature = "local_fs")]
fn reset_route_gitignores(route: &mut RepositoryWatchRoute) {
    let Some(root_dir) = route.root_dir.to_local_path() else {
        return;
    };
    route.gitignores = gitignores_for_directory(&root_dir);
    route.gitignore_checked_directories = HashSet::from([root_dir]);
    route.gitignore_snapshot_changed = true;
}

impl Entity for DirectoryWatcher {
    type Event = ();
}

impl SingletonEntity for DirectoryWatcher {}

/// Represents a file in a repository with its gitignore status.
#[derive(Debug, Clone)]
pub struct TargetFile {
    pub path: PathBuf,
    pub is_ignored: bool,
}

impl TargetFile {
    pub fn new(path: PathBuf, is_ignored: bool) -> Self {
        Self { path, is_ignored }
    }
}

impl Hash for TargetFile {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.path.hash(state);
        self.is_ignored.hash(state);
    }
}

impl PartialEq for TargetFile {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path && self.is_ignored == other.is_ignored
    }
}

impl Eq for TargetFile {}

impl PartialOrd for TargetFile {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TargetFile {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.path
            .cmp(&other.path)
            .then_with(|| self.is_ignored.cmp(&other.is_ignored))
    }
}

/// Changes detected in a repository.
#[derive(Debug, Clone, Default)]
pub struct RepositoryUpdate {
    /// Files that were added.
    pub added: HashSet<TargetFile>,

    /// Files whose contents were modified.
    pub modified: HashSet<TargetFile>,

    /// Files that were deleted.
    pub deleted: HashSet<TargetFile>,

    /// Files that were moved (to_path, from_path).
    pub moved: HashMap<TargetFile, TargetFile>,

    /// Whether a commit-related file changed (`.git/HEAD` or `.git/refs/heads/*`).
    pub commit_updated: bool,

    /// Whether the git index lock file was created or removed (`.git/index.lock`).
    pub index_lock_detected: bool,
}

impl RepositoryUpdate {
    /// Returns true if this update contains no changes.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.modified.is_empty()
            && self.deleted.is_empty()
            && self.moved.is_empty()
            && !self.commit_updated
            && !self.index_lock_detected
    }

    /// Iterator over all created and modified files.
    ///
    /// Most consumers don't care about the added-vs-modified distinction.
    pub fn added_or_modified(&self) -> impl Iterator<Item = &TargetFile> {
        self.added.iter().chain(self.modified.iter())
    }

    /// Owned iterator over all created and modified files.
    pub fn into_added_or_modified(self) -> impl Iterator<Item = TargetFile> {
        self.added.into_iter().chain(self.modified)
    }

    pub fn contains_added_or_modified(&self, file: &TargetFile) -> bool {
        self.added.contains(file) || self.modified.contains(file)
    }
}

/// An asynchronous task in a watched repository.
#[derive(Clone)]
enum Task {
    /// Perform an initial (or re-)scan for the given subscriber on the repository.
    Scan {
        repository: WeakModelHandle<Repository>,
        subscriber_id: SubscriberId,
    },
    #[cfg(feature = "local_fs")]
    /// Deliver an incremental update (filesystem changes) to a specific repository subscriber.
    Update {
        repository: WeakModelHandle<Repository>,
        subscriber_id: SubscriberId,
        update: RepositoryUpdate,
    },
}

impl Task {
    fn execute(
        self,
        ctx: &mut ModelContext<TaskQueue>,
    ) -> Option<Pin<Box<dyn Future<Output = ()> + Send>>> {
        match self {
            Task::Scan {
                repository,
                subscriber_id,
            } => {
                if let Some(repository) = repository.upgrade(ctx) {
                    repository.update(ctx, |repository, ctx| {
                        repository.scan_subscriber(subscriber_id, ctx)
                    })
                } else {
                    None
                }
            }
            #[cfg(feature = "local_fs")]
            Task::Update {
                repository,
                subscriber_id,
                update,
            } => {
                if let Some(repository) = repository.upgrade(ctx) {
                    repository.update(ctx, |repository, ctx| {
                        repository.notify_subscriber(subscriber_id, &update, ctx)
                    })
                } else {
                    None
                }
            }
        }
    }
}

#[derive(Clone)]
pub enum TaskQueueEvent {
    TaskEnqueued,
}

/// Lightweight task queue model for watched repositories. The [`RepositoryWatcher`] model uses this
/// to limit throughput for CPU- or disk-intensive update operations.
#[derive(Default)]
pub(crate) struct TaskQueue {
    /// Tasks which have not yet been executed.
    pending_tasks: VecDeque<Task>,
    active_tasks: usize,
}

impl TaskQueue {
    fn new(_ctx: &mut ModelContext<Self>) -> Self {
        Self::default()
    }

    /// Enqueue a new task.
    fn enqueue(&mut self, task: Task, ctx: &mut ModelContext<Self>) {
        self.pending_tasks.push_back(task);

        // Notify the watcher that a new task has been enqueued. This prevents circular model
        // updates by ensuring new tasks aren't immediately dequeued.
        ctx.emit(TaskQueueEvent::TaskEnqueued);
    }

    /// Advance through the queue by executing new tasks, up to the concurrency limit.
    fn advance(&mut self, ctx: &mut ModelContext<Self>) {
        while self.active_tasks < MAX_CONCURRENT_TASKS {
            let Some(task) = self.pending_tasks.pop_front() else {
                break;
            };

            if let Some(future) = task.execute(ctx) {
                self.active_tasks += 1;
                ctx.spawn(future, move |me, _, ctx| {
                    me.handle_task_completion(ctx);
                });
            }
        }
    }

    fn handle_task_completion(&mut self, ctx: &mut ModelContext<Self>) {
        self.active_tasks -= 1;
        self.advance(ctx);
    }

    /// Convenience helpers for enqueuing specific task kinds.
    pub(crate) fn enqueue_scan(
        &mut self,
        repository: WeakModelHandle<Repository>,
        subscriber_id: SubscriberId,
        ctx: &mut ModelContext<Self>,
    ) {
        self.enqueue(
            Task::Scan {
                repository,
                subscriber_id,
            },
            ctx,
        );
    }

    #[cfg(feature = "local_fs")]
    pub(crate) fn enqueue_incremental_update(
        &mut self,
        repository: WeakModelHandle<Repository>,
        subscriber_id: SubscriberId,
        update: RepositoryUpdate,
        ctx: &mut ModelContext<Self>,
    ) {
        self.enqueue(
            Task::Update {
                repository,
                subscriber_id,
                update,
            },
            ctx,
        );
    }
}

impl Entity for TaskQueue {
    type Event = TaskQueueEvent;
}

#[cfg(test)]
#[path = "watcher_tests.rs"]
mod tests;
