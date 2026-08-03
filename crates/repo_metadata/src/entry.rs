#![cfg_attr(not(feature = "local_fs"), allow(dead_code))]

#[cfg(feature = "local_fs")]
use futures::{future::BoxFuture, FutureExt as _};
#[cfg(feature = "local_fs")]
use futures_lite::StreamExt;
use ignore::gitignore::Gitignore;
#[cfg(feature = "local_fs")]
use std::collections::HashSet;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(feature = "local_fs")]
use std::sync::Arc;
use thiserror::Error;
use warp_util::standardized_path::StandardizedPath;

#[cfg(feature = "local_fs")]
use notify_debouncer_full::notify::WatchFilter;

/// Maximum file size allowed for treesitter parsing (3MB).
const MAX_FILE_SIZE: usize = 3 * 1000 * 1000;

/// Maximum number of files to load when lazy-loading a directory
pub const LAZY_LOAD_FILE_LIMIT: usize = 5000;

#[derive(Debug, Error)]
pub enum BuildTreeError {
    #[error("Repo size exceeded max file limit")]
    ExceededMaxFileLimit,
    #[error("File is ignored")]
    Ignored,
    #[error("IO error reading path.")]
    IOError(#[from] io::Error),
    #[error("Symlink is not supported")]
    Symlink,
    #[error("Maximum directory depth exceeded")]
    MaxDepthExceeded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IgnoredPathStrategy {
    /// Do not include any ignored files or folders
    Exclude,

    /// Lazy-load excluded directories
    IncludeLazy,

    /// Exclude all ignored files except for the ones in the given list
    IncludeOnly(Vec<String>),

    /// Add all of the ignored files into the tree
    Include,
}

/// Filesystem entry.
#[derive(Debug, Clone)]
pub enum Entry {
    File(FileMetadata),
    Directory(DirectoryEntry),
}

#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq)]
pub struct FileId(usize);

impl FileId {
    /// Constructs a new globally-unique file ID.
    #[allow(clippy::new_without_default)]
    pub(crate) fn new() -> FileId {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
        let raw = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        FileId(raw)
    }
}

impl Entry {
    pub fn path(&self) -> &StandardizedPath {
        match self {
            Self::File(file) => &file.path,
            Self::Directory(directory) => &directory.path,
        }
    }

    pub fn loaded(&self) -> bool {
        match self {
            Self::File(_) => true,
            Self::Directory(directory) => directory.loaded,
        }
    }

    pub fn ignored(&self) -> bool {
        match self {
            Self::File(file) => file.ignored,
            Self::Directory(directory) => directory.ignored,
        }
    }

    /// Builds a tree of entries from a given path, handling gitignored files and directories.
    /// After max_depth is reached, all children are lazy-loaded to prevent deeply nested trees.
    /// IgnoredPathStrategy determines what happens when ignored files are encountered.
    #[cfg(feature = "local_fs")]
    pub fn build_tree<'a>(
        path: impl Into<PathBuf>,
        files: &'a mut Vec<FileMetadata>,
        gitignores: &'a mut Vec<Gitignore>,
        mut remaining_file_quota: Option<&'a mut usize>,
        max_depth: usize,
        current_depth: usize,
        ignored_path_strategy: &'a IgnoredPathStrategy,
    ) -> BoxFuture<'a, Result<Self, BuildTreeError>> {
        let curr_path: PathBuf = path.into();
        async move {
            let is_dir = curr_path.is_dir();

            // Only ignore symlinks to directories. Symlinks to files are preserved (e.g. WARP.md).
            if curr_path.is_symlink() && is_dir {
                return Err(BuildTreeError::Symlink);
            }

            add_gitignore_for_directory(&curr_path, gitignores);

            let path_is_ignored = matches_gitignores(
                &curr_path,
                is_dir,
                &*gitignores,
                true, /* check_ancestors */
            ) || is_git_internal_path(&curr_path);

            // If we've reached the max depth, force lazy-loading even of non-ignored folders.
            let mut lazy_load = current_depth >= max_depth;

            if path_is_ignored {
                match ignored_path_strategy {
                    IgnoredPathStrategy::Exclude => {
                        return Err(BuildTreeError::Ignored);
                    }
                    IgnoredPathStrategy::IncludeOnly(patterns) => {
                        if let Some(file_name) = curr_path.file_name().and_then(|n| n.to_str()) {
                            if !patterns.iter().any(|pattern| file_name == pattern) {
                                return Err(BuildTreeError::Ignored);
                            }
                        }
                    }
                    IgnoredPathStrategy::IncludeLazy => {
                        // Git 不会在已被父规则排除的目录中继续读取子 `.gitignore`。
                        // 能重新包含的子项必须先重新包含父目录，因此当前目录被忽略
                        // 时可以安全地延迟加载，避免一个无关的 `!` 规则让 target /
                        // node_modules 等大型树在启动时全量扫描。
                        lazy_load = true;
                    }
                    IgnoredPathStrategy::Include => {}
                }
            }

            if is_dir {
                if lazy_load {
                    return Ok(Self::Directory(DirectoryEntry {
                        children: vec![],
                        path: StandardizedPath::from_local_absolute_unchecked(&curr_path),
                        ignored: path_is_ignored,
                        loaded: false,
                    }));
                }

                // If the path is a directory, process all the children under it.
                let mut entries = async_fs::read_dir(&curr_path).await?;
                let mut children = Vec::new();

                while let Some(entry) = entries.next().await {
                    if remaining_file_quota
                        .as_ref()
                        .is_some_and(|x| **x < children.len())
                    {
                        return Err(BuildTreeError::ExceededMaxFileLimit);
                    }

                    if let Some(entry) = match entry {
                        Ok(entry) => {
                            let entry_path = entry.path();

                            // Skip symlinks to folders before canonicalization to prevent duplicates.
                            // If it's a symlink to a file, we keep the path as is since canonicalization would
                            // point its path to the actual file.
                            let canonical_path = if entry_path.is_symlink() {
                                if entry_path.is_dir() {
                                    None
                                } else {
                                    Some(entry_path)
                                }
                            } else {
                                dunce::canonicalize(entry_path).ok()
                            };

                            if let Some(canonical_path) = canonical_path {
                                match Entry::build_tree(
                                    canonical_path,
                                    files,
                                    gitignores,
                                    remaining_file_quota.as_deref_mut(),
                                    max_depth,
                                    current_depth + 1,
                                    ignored_path_strategy,
                                )
                                .await
                                {
                                    Ok(entry) => Some(entry),
                                    Err(BuildTreeError::ExceededMaxFileLimit) => {
                                        return Err(BuildTreeError::ExceededMaxFileLimit)
                                    }
                                    Err(_) => None,
                                }
                            } else {
                                None
                            }
                        }
                        Err(_) => None,
                    } {
                        children.push(entry);
                    }
                }

                Ok(Self::Directory(DirectoryEntry {
                    children,
                    path: StandardizedPath::from_local_absolute_unchecked(&curr_path),
                    ignored: path_is_ignored,
                    loaded: true,
                }))
            } else if curr_path.is_file() {
                if let Some(remaining_file_quota) = remaining_file_quota {
                    if *remaining_file_quota == 0 {
                        return Err(BuildTreeError::ExceededMaxFileLimit);
                    }

                    *remaining_file_quota -= 1
                }
                let metadata = FileMetadata::new(curr_path, path_is_ignored);
                files.push(metadata.clone());
                Ok(Self::File(metadata))
            } else {
                Err(BuildTreeError::Symlink)
            }
        }
        .boxed()
    }

    /// Finds an entry based on path
    pub fn find_mut(&mut self, path: &Path) -> Option<&mut Entry> {
        let std_path = StandardizedPath::try_from_local(path).ok()?;
        self.find_mut_by_std_path(&std_path)
    }

    fn find_mut_by_std_path(&mut self, path: &StandardizedPath) -> Option<&mut Entry> {
        if self.path() == path {
            return Some(self);
        }

        if let Self::Directory(directory) = self {
            if !path.starts_with(&directory.path) {
                // Target is not descendant of directory.
                return None;
            }

            for child in directory.children.iter_mut() {
                if let Some(entry) = child.find_mut_by_std_path(path) {
                    return Some(entry);
                }
            }
        }

        None
    }

    /// Loads an unloaded directory
    #[cfg(feature = "local_fs")]
    pub fn load<'a>(
        &'a mut self,
        gitignores: &'a mut Vec<Gitignore>,
    ) -> BoxFuture<'a, Result<(), BuildTreeError>> {
        async move {
            // TODO: Consider a similar `unload` method if we run into performance issues.
            let Self::Directory(directory) = self else {
                return Ok(());
            };

            let mut remaining_file_quota = LAZY_LOAD_FILE_LIMIT;
            let mut files = Vec::new();

            let result = Entry::build_tree(
                directory.path.to_local_path_lossy(),
                &mut files,
                gitignores,
                Some(&mut remaining_file_quota),
                1, /* max_depth */
                0, /* current_depth */
                &IgnoredPathStrategy::Include,
            )
            .await;

            result.map(|entry| match entry {
                Entry::Directory(entry) => {
                    *directory = entry;
                }
                Entry::File(_) => {
                    log::error!("Called load on a directory but a file entry was returned");
                }
            })
        }
        .boxed()
    }

    /// Removes the entry corresponding to the given target path, if any.
    pub fn remove(&mut self, target_path: &Path) -> Option<FileMetadata> {
        let std_path = StandardizedPath::try_from_local(target_path).ok()?;
        self.remove_by_std_path(&std_path)
    }

    fn remove_by_std_path(&mut self, target_path: &StandardizedPath) -> Option<FileMetadata> {
        let Self::Directory(directory) = self else {
            // We should never hit this condition - we only end up recursing into directories given
            // that recursion only occurs when `target_path` is a descendant of `directory.path`
            // but not a direct child.
            return None;
        };
        if !target_path.starts_with(&directory.path) {
            // Target is not descendant of directory.
            return None;
        }
        for (index, child) in directory.children.iter_mut().enumerate() {
            if child.path() == target_path {
                // If the child's path is the target path, remove the child.
                return match directory.children.remove(index) {
                    Entry::Directory(_) => None,
                    Entry::File(metadata) => Some(metadata),
                };
            } else if target_path.starts_with(child.path()) {
                // Child is a descendant of the target path, so recurse.
                return child.remove_by_std_path(target_path);
            }
        }

        log::debug!("target path not found under the current directory node");
        None
    }
}

pub fn is_git_internal_path(path: &Path) -> bool {
    path.components().any(|component| {
        if let Component::Normal(name) = component {
            name == ".git"
        } else {
            false
        }
    })
}

/// Returns true if a path matches any of the gitignores.
///
/// For example, if the directory `/target` is ignored:
/// - If `check_ancestors` is true, then `/target/debug` will match.
/// - If `check_ancestors` is false, then `/target/debug` will not match.
pub fn matches_gitignores(
    path: &Path,
    is_dir: bool,
    gitignores: &[Gitignore],
    check_ancestors: bool,
) -> bool {
    let mut ignored = false;
    for gitignore in gitignores {
        if let Ok(relative_path) = path.strip_prefix(gitignore.path()) {
            // `matched_path_or_any_parents` panics if the path has a root.
            // If not on windows, we allow paths with a root if the gitignore path is empty (since this denotes a global gitignore).
            if relative_path.has_root() && (cfg!(windows) || gitignore.path() != Path::new("")) {
                continue;
            }

            let matched = if check_ancestors {
                gitignore.matched_path_or_any_parents(relative_path, is_dir)
            } else {
                gitignore.matched(relative_path, is_dir)
            };
            if !matched.is_none() {
                // Git 的优先级是越靠近文件的规则越高。调用方按
                // global → 根目录 → 嵌套目录的顺序保存 matcher，因此后面的
                // 非空匹配可以覆盖前面的 Ignore 或 Whitelist。
                ignored = matched.is_ignore();
            }
        }
    }
    ignored
}

/// Returns the path components after `.git` in a git-internal path,
/// skipping the worktree indirection (`.git/worktrees/<name>/…`) if present.
/// Returns `None` if the path has no `.git` component or nothing follows it.
fn git_suffix_components(path: &Path) -> Option<Vec<Component<'_>>> {
    let components: Vec<_> = path.components().collect();
    let git_index = components.iter().position(|c| c.as_os_str() == ".git")?;

    let after_git = &components[git_index + 1..];
    if after_git.is_empty() {
        return None;
    }

    // For worktrees the layout is `.git/worktrees/<name>/…`.
    // Skip the `worktrees/<name>` prefix so callers see the same
    // logical structure as a normal repo.
    if after_git.first().map(|c| c.as_os_str()) == Some(std::ffi::OsStr::new("worktrees"))
        && after_git.len() >= 3
    {
        // after_git[0] = "worktrees", [1] = <name>, [2..] = actual content
        return Some(after_git[2..].to_vec());
    }

    Some(after_git.to_vec())
}

/// Given a path like `.../repo/.git/worktrees/foo/HEAD`, returns
/// `.../repo/.git/worktrees/foo`. Returns `None` for non-worktree paths.
pub(crate) fn extract_worktree_git_dir(path: &Path) -> Option<PathBuf> {
    let components: Vec<_> = path.components().collect();
    let git_index = components.iter().position(|c| c.as_os_str() == ".git")?;
    let after_git = &components[git_index + 1..];
    if after_git.len() >= 3
        && after_git
            .first()
            .map(|c| c.as_os_str() == "worktrees")
            .unwrap_or(false)
    {
        // Rebuild: everything up to and including .git/worktrees/<name>
        Some(components[..git_index + 3].iter().collect())
    } else {
        None
    }
}

/// Returns `true` for shared ref paths that live directly in the common
/// `.git` directory and should be broadcast to all repos sharing it.
/// Currently this means `.git/refs/heads/*` (not under `.git/worktrees/`).
pub(crate) fn is_shared_git_ref(path: &Path) -> bool {
    if extract_worktree_git_dir(path).is_some() {
        return false;
    }
    let components: Vec<_> = path.components().collect();
    let Some(git_index) = components.iter().position(|c| c.as_os_str() == ".git") else {
        return false;
    };
    let after_git = &components[git_index + 1..];
    after_git
        .first()
        .map(|c| c.as_os_str() == "refs")
        .unwrap_or(false)
        && after_git
            .get(1)
            .map(|c| c.as_os_str() == "heads")
            .unwrap_or(false)
}

/// Returns true for `.git/HEAD` and `.git/refs/heads/*`
/// (and their worktree equivalents `.git/worktrees/*/HEAD`, etc.).
pub(crate) fn is_commit_related_git_file(path: &Path) -> bool {
    let Some(suffix) = git_suffix_components(path) else {
        return false;
    };
    match suffix.first().map(|c| c.as_os_str()) {
        Some(name) if name == "HEAD" => true,
        Some(name) if name == "refs" => {
            suffix.get(1).map(|c| c.as_os_str()) == Some(std::ffi::OsStr::new("heads"))
        }
        _ => false,
    }
}

/// Returns true for `.git/index.lock`
/// (and its worktree equivalent `.git/worktrees/*/index.lock`).
pub(crate) fn is_index_lock_file(path: &Path) -> bool {
    let Some(suffix) = git_suffix_components(path) else {
        return false;
    };
    suffix.len() == 1 && suffix[0].as_os_str() == "index.lock"
}

/// Determines if a git-related path should be ignored by the filesystem watcher.
///
/// Uses an allowlist approach: only commit-related files (HEAD, refs/heads/*)
/// and the index lock file are allowed through. Everything else inside `.git/`
/// is ignored.
pub fn should_ignore_git_path(path: &Path) -> bool {
    if !is_git_internal_path(path) {
        return false; // Not a git path, don't ignore
    }
    // Ignore everything inside .git/ except the allowlisted patterns.
    !is_commit_related_git_file(path) && !is_index_lock_file(path)
}

/// 判断仓库 watcher 是否应递归进入 `path`。
///
/// `.git/` 内部路径遵循 watcher allowlist；目录 symlink 会被剪枝，避免
/// 递归跟随到仓库外的大型目录树。被监听的根目录本身仍允许是 symlink。
#[cfg(feature = "local_fs")]
pub fn should_watch_repo_directory(path: &Path, repo_root: &Path) -> bool {
    if is_within_symlink(path, repo_root) {
        return false;
    }

    // .git 与目录 symlink 会造成不可恢复的高噪声/越界递归；其余路径都交给
    // 后台树扫描按完整 Git ignore 规则判定。不能在入口按当前快照剪枝，
    // 否则 `.gitignore` 的否定规则或后续修改会让重新包含的子项永远收不到事件。
    !should_ignore_git_path(path)
}

/// 判断 `path` 本身或它的祖先是否是 symlink。
///
/// 递归 watcher 需要这个检查保持单调性：如果一个 symlink 目录被拒绝，
/// 那么它的后代也必须被拒绝，即便后代路径本身不是 symlink。
#[cfg(feature = "local_fs")]
fn is_within_symlink(path: &Path, repo_root: &Path) -> bool {
    path.ancestors()
        .take_while(|ancestor| *ancestor != repo_root && ancestor.starts_with(repo_root))
        .any(|ancestor| {
            std::fs::symlink_metadata(ancestor)
                .is_ok_and(|metadata| metadata.file_type().is_symlink())
        })
}

/// 创建仓库递归监听器使用的过滤器。
///
/// 过滤器在文件监听后台线程执行，可以安全地查询目录类型；UI 线程只接收已
/// 过滤和去抖后的事件。
#[cfg(feature = "local_fs")]
pub fn repo_watch_filter(repo_root: PathBuf) -> WatchFilter {
    WatchFilter::with_filter(Arc::new(move |path| {
        should_watch_repo_directory(path, &repo_root)
    }))
}

#[cfg(feature = "local_fs")]
pub fn path_passes_filters(path: &Path, gitignores: &[Gitignore]) -> bool {
    let to_check_path = if path.exists() {
        match dunce::canonicalize(path) {
            Ok(canonical_path) => canonical_path,
            Err(_) => return false,
        }
    } else {
        path.to_path_buf()
    };

    !matches_gitignores(
        &to_check_path,
        to_check_path.is_dir(),
        gitignores,
        true, /* check_ancestors */
    ) && !should_ignore_git_path(&to_check_path)
}

/// Determines whether a file should be parsed by a treesitter query. For now the main criteria is it shouldn't
/// exceed the given file size limit.
#[cfg(feature = "local_fs")]
pub fn is_file_parsable(path: &Path) -> Result<bool, io::Error> {
    std::fs::metadata(path).map(|metadata| (metadata.len() as usize) < MAX_FILE_SIZE)
}

#[cfg(feature = "local_fs")]
pub fn gitignores_for_directory(directory_path: &Path) -> Vec<Gitignore> {
    let mut gitignores = Vec::new();
    let (global_gitignore, _) = Gitignore::global();
    if !global_gitignore.is_empty() {
        gitignores.push(global_gitignore);
    }
    add_gitignore_for_directory(directory_path, &mut gitignores);
    gitignores
}

/// 将仓库根目录到事件路径父目录的 `.gitignore` 规则补进快照。
///
/// 该函数只在后台扫描中调用。惰性目录可能尚未被树构建访问过，但其祖先规则
/// 依然必须参与新增、删除和重命名事件的 Git ignore 判定。`checked_directories`
/// 同时缓存不存在 `.gitignore` 的目录，避免高频文件事件反复探测同一路径。
#[cfg(feature = "local_fs")]
pub fn add_gitignores_for_path(
    repo_root: &Path,
    path: &Path,
    is_dir: bool,
    gitignores: &mut Vec<Gitignore>,
    checked_directories: &mut HashSet<PathBuf>,
) {
    let directory = if is_dir {
        path
    } else {
        path.parent().unwrap_or(repo_root)
    };
    let Ok(relative_directory) = directory.strip_prefix(repo_root) else {
        return;
    };

    let mut current_directory = repo_root.to_path_buf();
    add_gitignore_for_directory_if_needed(&current_directory, gitignores, checked_directories);
    for component in relative_directory.components() {
        if let Component::Normal(component) = component {
            current_directory.push(component);
            add_gitignore_for_directory_if_needed(
                &current_directory,
                gitignores,
                checked_directories,
            );
        }
    }
}

/// 丢弃某个目录及其后代的规则探测缓存。
///
/// 目录被删除或移动后，原 `.gitignore` matcher 不能继续用于未来重新创建的路径。
/// 调用方随后会按需重新加载该路径的规则。
#[cfg(feature = "local_fs")]
pub fn invalidate_gitignores_under_path(
    path: &Path,
    gitignores: &mut Vec<Gitignore>,
    checked_directories: &mut HashSet<PathBuf>,
) {
    gitignores.retain(|gitignore| {
        let gitignore_path = gitignore.path();
        gitignore_path.as_os_str().is_empty() || !gitignore_path.starts_with(path)
    });
    checked_directories.retain(|directory| !directory.starts_with(path));
}

#[cfg(feature = "local_fs")]
fn add_gitignore_for_directory_if_needed(
    directory_path: &Path,
    gitignores: &mut Vec<Gitignore>,
    checked_directories: &mut HashSet<PathBuf>,
) {
    if checked_directories.insert(directory_path.to_path_buf()) {
        add_gitignore_for_directory(directory_path, gitignores);
    }
}

/// 将目录自己的 `.gitignore` 加入快照，避免对同一目录重复构建 matcher。
#[cfg(feature = "local_fs")]
fn add_gitignore_for_directory(directory_path: &Path, gitignores: &mut Vec<Gitignore>) {
    if gitignores
        .iter()
        .any(|gitignore| gitignore.path() == directory_path)
    {
        return;
    }

    let gitignore_path = directory_path.join(".gitignore");
    if gitignore_path.exists() {
        let (gitignore, _) = Gitignore::new(gitignore_path);
        gitignores.push(gitignore);
    }
}

#[derive(Debug, Clone)]
pub struct FileMetadata {
    /// Absolute path to the file.
    pub path: StandardizedPath,
    pub file_id: FileId,
    pub extension: Option<String>,
    pub ignored: bool,
}

impl FileMetadata {
    pub fn new(path: PathBuf, ignored: bool) -> Self {
        let path_extension = path.extension().and_then(|extension| extension.to_str());
        let file_id = FileId::new();
        let std_path = StandardizedPath::from_local_absolute_unchecked(&path);
        Self {
            file_id,
            extension: path_extension.map(str::to_string),
            path: std_path,
            ignored,
        }
    }

    /// Construct from a [`StandardizedPath`] directly, without filesystem I/O.
    pub fn from_standardized(path: StandardizedPath, ignored: bool) -> Self {
        let file_id = FileId::new();
        let extension = path.extension().map(|s| s.to_owned());
        Self {
            file_id,
            extension,
            path,
            ignored,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DirectoryEntry {
    /// Absolute path to the directory.
    pub path: StandardizedPath,
    pub children: Vec<Entry>,
    pub ignored: bool,
    pub loaded: bool,
}

impl DirectoryEntry {
    pub fn find_or_insert_child(&mut self, target_path: &Path) -> Option<&mut Entry> {
        let std_path = StandardizedPath::try_from_local(target_path).ok()?;

        // First, try to find the child's position
        if let Some(index) = self
            .children
            .iter()
            .position(|child| *child.path() == std_path)
        {
            // Child exists, return a mutable reference to it
            return Some(&mut self.children[index]);
        }

        // Child not found, create new entry if the path is valid
        let new_entry = if target_path.is_dir() {
            Entry::Directory(DirectoryEntry {
                children: vec![],
                path: std_path,
                loaded: false,
                ignored: false,
            })
        } else if target_path.is_file() {
            Entry::File(FileMetadata {
                path: std_path.clone(),
                file_id: FileId::new(),
                extension: std_path.extension().map(|s| s.to_owned()),
                ignored: false,
            })
        } else {
            // Cannot insert child since target_path is neither a file or a directory.
            return None;
        };

        // Insert the new entry and return a mutable reference to it
        self.children.push(new_entry);
        self.children.last_mut()
    }

    /// Similar to find_or_insert_child but specifically for creating directory entries.
    /// This is used when we know the path should be a directory (e.g., when ensuring parent directories exist).
    pub fn find_or_insert_directory(&mut self, target_path: &Path) -> Option<&mut Entry> {
        let std_path = StandardizedPath::try_from_local(target_path).ok()?;

        // First, try to find the child's position
        if let Some(index) = self
            .children
            .iter()
            .position(|child| *child.path() == std_path)
        {
            // Child exists, return a mutable reference to it
            return Some(&mut self.children[index]);
        }

        // Child not found, create new directory entry
        let new_entry = Entry::Directory(DirectoryEntry {
            children: vec![],
            path: std_path,
            ignored: false,
            loaded: false,
        });

        // Insert the new entry and return a mutable reference to it
        self.children.push(new_entry);
        self.children.last_mut()
    }
}

#[cfg(test)]
#[path = "entry_test.rs"]
mod tests;
