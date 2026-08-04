use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::error::{Result, TransportError};
use crate::sftp::Sftp;
use crate::transfer::{TransferController, TransferStatus};
use crate::types::{DirEntry, FilePermissions, FileType, Metadata, RenameOptions};

const PARTIAL_CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);

/// 统一的异步远程文件系统接口。
#[async_trait]
pub trait RemoteFs: Send + Sync {
    async fn home_dir(&self) -> Result<PathBuf>;
    async fn list_dir(&self, path: &Path) -> Result<Vec<DirEntry>>;
    async fn stat(&self, path: &Path) -> Result<Metadata>;
    async fn mkdir(&self, path: &Path) -> Result<()>;
    async fn remove_file(&self, path: &Path) -> Result<()>;
    async fn remove_dir(&self, path: &Path) -> Result<()>;
    async fn remove_dir_all(&self, path: &Path) -> Result<()>;
    async fn rename(&self, old_path: &Path, new_path: &Path, options: RenameOptions) -> Result<()>;
    async fn realpath(&self, path: &Path) -> Result<PathBuf>;
    async fn upload_file(
        &self,
        local_path: &Path,
        remote_path: &Path,
        controller: Option<Arc<TransferController>>,
    ) -> Result<()>;
    async fn download_file(
        &self,
        remote_path: &Path,
        local_path: &Path,
        controller: Option<Arc<TransferController>>,
    ) -> Result<()>;
    async fn upload_directory(
        &self,
        local_path: &Path,
        remote_path: &Path,
        controller: Option<Arc<TransferController>>,
    ) -> Result<()>;
    async fn download_directory(
        &self,
        remote_path: &Path,
        local_path: &Path,
        controller: Option<Arc<TransferController>>,
    ) -> Result<()>;
}

#[async_trait]
impl RemoteFs for Sftp {
    async fn home_dir(&self) -> Result<PathBuf> {
        self.realpath(Path::new(".")).await
    }

    async fn list_dir(&self, path: &Path) -> Result<Vec<DirEntry>> {
        self.read_dir(path).await
    }

    async fn stat(&self, path: &Path) -> Result<Metadata> {
        Sftp::stat(self, path).await
    }

    async fn mkdir(&self, path: &Path) -> Result<()> {
        self.create_dir(path).await
    }

    async fn remove_file(&self, path: &Path) -> Result<()> {
        Sftp::remove_file(self, path).await
    }

    async fn remove_dir(&self, path: &Path) -> Result<()> {
        Sftp::remove_dir(self, path).await
    }

    async fn remove_dir_all(&self, path: &Path) -> Result<()> {
        remove_remote_dir_all(self, path).await
    }

    async fn rename(&self, old_path: &Path, new_path: &Path, options: RenameOptions) -> Result<()> {
        Sftp::rename(self, old_path, new_path, options).await
    }

    async fn realpath(&self, path: &Path) -> Result<PathBuf> {
        Sftp::realpath(self, path).await
    }

    async fn upload_file(
        &self,
        local_path: &Path,
        remote_path: &Path,
        controller: Option<Arc<TransferController>>,
    ) -> Result<()> {
        let total = tokio::fs::metadata(local_path).await?.len();
        begin_transfer(controller.as_deref(), total, 1);
        let partial = partial_path(remote_path);
        let result = async {
            self.upload_stream(local_path, &partial, controller.as_deref())
                .await?;
            if let Some(controller) = controller.as_deref() {
                controller.wait_for_transfer_ready().await?;
            }
            if self.try_exists(remote_path).await? {
                self.remove_file(remote_path).await?;
            }
            Sftp::rename(self, &partial, remote_path, RenameOptions::default()).await
        }
        .await;
        if result.is_err() {
            let _ = tokio::time::timeout(PARTIAL_CLEANUP_TIMEOUT, self.remove_file(&partial)).await;
        }
        finish_transfer(controller.as_deref(), result)
    }

    async fn download_file(
        &self,
        remote_path: &Path,
        local_path: &Path,
        controller: Option<Arc<TransferController>>,
    ) -> Result<()> {
        let total = Sftp::stat(self, remote_path).await?.size;
        begin_transfer(controller.as_deref(), total, 1);
        let partial = partial_path(local_path);
        if let Some(parent) = partial.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let result = async {
            self.download_stream(remote_path, &partial, controller.as_deref())
                .await?;
            if let Some(controller) = controller.as_deref() {
                controller.wait_for_transfer_ready().await?;
            }
            if tokio::fs::try_exists(local_path).await? {
                tokio::fs::remove_file(local_path).await?;
            }
            tokio::fs::rename(&partial, local_path).await?;
            Ok(())
        }
        .await;
        if result.is_err() {
            let _ = tokio::fs::remove_file(&partial).await;
        }
        finish_transfer(controller.as_deref(), result)
    }

    async fn upload_directory(
        &self,
        local_path: &Path,
        remote_path: &Path,
        controller: Option<Arc<TransferController>>,
    ) -> Result<()> {
        let items = collect_local_tree(local_path, remote_path).await?;
        let total_bytes = items.iter().map(|item| item.size).sum();
        let total_files = items.iter().filter(|item| !item.directory).count() as u64;
        begin_transfer(controller.as_deref(), total_bytes, total_files);
        let result = async {
            if !self.try_exists(remote_path).await? {
                self.create_dir(remote_path).await?;
            }
            let mut completed = 0;
            for item in items {
                if item.directory {
                    if !self.try_exists(&item.remote).await? {
                        self.create_dir(&item.remote).await?;
                    }
                    continue;
                }
                let partial = partial_path(&item.remote);
                let item_result: Result<()> = async {
                    self.upload_stream(&item.local, &partial, controller.as_deref())
                        .await?;
                    if let Some(controller) = controller.as_deref() {
                        controller.wait_for_transfer_ready().await?;
                    }
                    if self.try_exists(&item.remote).await? {
                        self.remove_file(&item.remote).await?;
                    }
                    self.rename(&partial, &item.remote, RenameOptions::default())
                        .await
                }
                .await;
                if let Err(error) = item_result {
                    let _ =
                        tokio::time::timeout(PARTIAL_CLEANUP_TIMEOUT, self.remove_file(&partial))
                            .await;
                    return Err(error);
                }
                completed += 1;
                if let Some(controller) = controller.as_deref() {
                    controller.update_item_progress(completed, total_files);
                }
            }
            Ok(())
        }
        .await;
        finish_transfer(controller.as_deref(), result)
    }

    async fn download_directory(
        &self,
        remote_path: &Path,
        local_path: &Path,
        controller: Option<Arc<TransferController>>,
    ) -> Result<()> {
        let items = collect_remote_tree(self, remote_path, local_path).await?;
        let total_bytes = items.iter().map(|item| item.size).sum();
        let total_files = items.iter().filter(|item| !item.directory).count() as u64;
        begin_transfer(controller.as_deref(), total_bytes, total_files);
        let result = async {
            tokio::fs::create_dir_all(local_path).await?;
            let mut completed = 0;
            for item in items {
                if item.directory {
                    tokio::fs::create_dir_all(&item.local).await?;
                    continue;
                }
                if let Some(parent) = item.local.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                let partial = partial_path(&item.local);
                let item_result: Result<()> = async {
                    self.download_stream(&item.remote, &partial, controller.as_deref())
                        .await?;
                    if let Some(controller) = controller.as_deref() {
                        controller.wait_for_transfer_ready().await?;
                    }
                    if tokio::fs::try_exists(&item.local).await? {
                        tokio::fs::remove_file(&item.local).await?;
                    }
                    tokio::fs::rename(&partial, &item.local)
                        .await
                        .map_err(TransportError::from)
                }
                .await;
                if let Err(error) = item_result {
                    let _ = tokio::fs::remove_file(&partial).await;
                    return Err(error);
                }
                completed += 1;
                if let Some(controller) = controller.as_deref() {
                    controller.update_item_progress(completed, total_files);
                }
            }
            Ok(())
        }
        .await;
        finish_transfer(controller.as_deref(), result)
    }
}

#[derive(Debug)]
struct TransferItem {
    local: PathBuf,
    remote: PathBuf,
    directory: bool,
    size: u64,
}

async fn collect_local_tree(local_root: &Path, remote_root: &Path) -> Result<Vec<TransferItem>> {
    let mut items = Vec::new();
    let mut pending = vec![(local_root.to_path_buf(), remote_root.to_path_buf())];
    while let Some((local, remote)) = pending.pop() {
        let metadata = tokio::fs::symlink_metadata(&local).await?;
        if metadata.is_dir() {
            items.push(TransferItem {
                local: local.clone(),
                remote: remote.clone(),
                directory: true,
                size: 0,
            });
            let mut entries = tokio::fs::read_dir(&local).await?;
            while let Some(entry) = entries.next_entry().await? {
                pending.push((entry.path(), remote.join(entry.file_name())));
            }
        } else {
            items.push(TransferItem {
                local,
                remote,
                directory: false,
                size: metadata.len(),
            });
        }
    }
    items.sort_by(|left, right| left.remote.cmp(&right.remote));
    Ok(items)
}

async fn collect_remote_tree(
    fs: &Sftp,
    remote_root: &Path,
    local_root: &Path,
) -> Result<Vec<TransferItem>> {
    let mut items = Vec::new();
    let mut pending = vec![(remote_root.to_path_buf(), local_root.to_path_buf())];
    while let Some((remote, local)) = pending.pop() {
        let metadata = fs.lstat(&remote).await?;
        if metadata.file_type == FileType::Dir {
            items.push(TransferItem {
                local: local.clone(),
                remote: remote.clone(),
                directory: true,
                size: 0,
            });
            for entry in fs.read_dir(&remote).await? {
                pending.push((entry.path, local.join(entry.name)));
            }
        } else {
            items.push(TransferItem {
                local,
                remote,
                directory: false,
                size: metadata.size,
            });
        }
    }
    items.sort_by(|left, right| left.remote.cmp(&right.remote));
    Ok(items)
}

async fn remove_remote_dir_all(fs: &Sftp, path: &Path) -> Result<()> {
    let mut pending = vec![(path.to_path_buf(), false)];
    while let Some((current, visited)) = pending.pop() {
        let metadata = fs.lstat(&current).await?;
        if metadata.file_type != FileType::Dir {
            fs.remove_file(&current).await?;
            continue;
        }
        if visited {
            fs.remove_dir(&current).await?;
            continue;
        }
        pending.push((current.clone(), true));
        for entry in fs.read_dir(&current).await? {
            pending.push((entry.path, false));
        }
    }
    Ok(())
}

fn partial_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "transfer".to_string());
    path.parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!(".{name}.zora-partial"))
}

fn begin_transfer(controller: Option<&TransferController>, total_bytes: u64, total_files: u64) {
    if let Some(controller) = controller {
        if matches!(
            controller.snapshot().status,
            TransferStatus::Pending | TransferStatus::Running | TransferStatus::Paused
        ) {
            controller.start(total_bytes, total_files);
        }
    }
}

fn finish_transfer(controller: Option<&TransferController>, result: Result<()>) -> Result<()> {
    match result {
        Ok(()) => {
            if let Some(controller) = controller {
                if controller.snapshot().status == TransferStatus::Cancelled {
                    return Err(TransportError::Cancelled);
                }
                controller.complete();
            }
            Ok(())
        }
        Err(error) => {
            if let Some(controller) = controller {
                if controller.snapshot().status != TransferStatus::Cancelled {
                    controller.fail(error.to_string());
                }
            }
            Err(error)
        }
    }
}

/// 用于 UI 单元测试和离线演示的本地远程文件系统。
#[derive(Clone, Debug)]
pub struct LocalRemoteFs {
    root: Arc<PathBuf>,
}

impl LocalRemoteFs {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root: Arc::new(root),
        }
    }

    fn local_path(&self, remote: &Path) -> Result<PathBuf> {
        let remote = remote.to_string_lossy().replace('\\', "/");
        let mut path = self.root.as_ref().clone();
        for component in remote.split('/') {
            if component.is_empty() || component == "." {
                continue;
            }
            if component == ".." {
                return Err(TransportError::InvalidPath(remote));
            }
            path.push(component);
        }
        Ok(path)
    }

    fn remote_path(&self, local: &Path) -> Result<PathBuf> {
        let relative = local
            .strip_prefix(self.root.as_ref())
            .map_err(|_| TransportError::InvalidPath(local.display().to_string()))?;
        Ok(PathBuf::from("/").join(relative))
    }
}

#[async_trait]
impl RemoteFs for LocalRemoteFs {
    async fn home_dir(&self) -> Result<PathBuf> {
        Ok(PathBuf::from("/"))
    }

    async fn list_dir(&self, path: &Path) -> Result<Vec<DirEntry>> {
        let local = self.local_path(path)?;
        let mut result = Vec::new();
        let mut entries = tokio::fs::read_dir(&local).await?;
        while let Some(entry) = entries.next_entry().await? {
            let metadata = tokio::fs::symlink_metadata(entry.path()).await?;
            result.push(DirEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                path: self.remote_path(&entry.path())?,
                metadata: metadata_from_local(&metadata),
            });
        }
        result.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(result)
    }

    async fn stat(&self, path: &Path) -> Result<Metadata> {
        Ok(metadata_from_local(
            &tokio::fs::symlink_metadata(self.local_path(path)?).await?,
        ))
    }

    async fn mkdir(&self, path: &Path) -> Result<()> {
        tokio::fs::create_dir(self.local_path(path)?)
            .await
            .map_err(Into::into)
    }

    async fn remove_file(&self, path: &Path) -> Result<()> {
        tokio::fs::remove_file(self.local_path(path)?)
            .await
            .map_err(Into::into)
    }

    async fn remove_dir(&self, path: &Path) -> Result<()> {
        tokio::fs::remove_dir(self.local_path(path)?)
            .await
            .map_err(Into::into)
    }

    async fn remove_dir_all(&self, path: &Path) -> Result<()> {
        tokio::fs::remove_dir_all(self.local_path(path)?)
            .await
            .map_err(Into::into)
    }

    async fn rename(&self, old_path: &Path, new_path: &Path, options: RenameOptions) -> Result<()> {
        let old = self.local_path(old_path)?;
        let new = self.local_path(new_path)?;
        if options.overwrite && tokio::fs::try_exists(&new).await? {
            let metadata = tokio::fs::symlink_metadata(&new).await?;
            if metadata.is_dir() {
                tokio::fs::remove_dir_all(&new).await?;
            } else {
                tokio::fs::remove_file(&new).await?;
            }
        }
        tokio::fs::rename(old, new).await.map_err(Into::into)
    }

    async fn realpath(&self, path: &Path) -> Result<PathBuf> {
        let local = tokio::fs::canonicalize(self.local_path(path)?).await?;
        self.remote_path(&local)
    }

    async fn upload_file(
        &self,
        local_path: &Path,
        remote_path: &Path,
        controller: Option<Arc<TransferController>>,
    ) -> Result<()> {
        let total = tokio::fs::metadata(local_path).await?.len();
        begin_transfer(controller.as_deref(), total, 1);
        let target = self.local_path(remote_path)?;
        let partial = partial_path(&target);
        if let Some(parent) = partial.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let result = async {
            copy_local_file(local_path, &partial, controller.as_deref()).await?;
            if let Some(controller) = controller.as_deref() {
                controller.wait_for_transfer_ready().await?;
            }
            if tokio::fs::try_exists(&target).await? {
                tokio::fs::remove_file(&target).await?;
            }
            tokio::fs::rename(&partial, target).await?;
            Ok(())
        }
        .await;
        if result.is_err() {
            let _ = tokio::fs::remove_file(&partial).await;
        }
        finish_transfer(controller.as_deref(), result)
    }

    async fn download_file(
        &self,
        remote_path: &Path,
        local_path: &Path,
        controller: Option<Arc<TransferController>>,
    ) -> Result<()> {
        let source = self.local_path(remote_path)?;
        let total = tokio::fs::metadata(&source).await?.len();
        begin_transfer(controller.as_deref(), total, 1);
        let partial = partial_path(local_path);
        if let Some(parent) = partial.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let result = async {
            copy_local_file(&source, &partial, controller.as_deref()).await?;
            if let Some(controller) = controller.as_deref() {
                controller.wait_for_transfer_ready().await?;
            }
            if tokio::fs::try_exists(local_path).await? {
                tokio::fs::remove_file(local_path).await?;
            }
            tokio::fs::rename(&partial, local_path).await?;
            Ok(())
        }
        .await;
        if result.is_err() {
            let _ = tokio::fs::remove_file(&partial).await;
        }
        finish_transfer(controller.as_deref(), result)
    }

    async fn upload_directory(
        &self,
        local_path: &Path,
        remote_path: &Path,
        controller: Option<Arc<TransferController>>,
    ) -> Result<()> {
        let items = collect_local_tree(local_path, remote_path).await?;
        let total_bytes = items.iter().map(|item| item.size).sum();
        let total_files = items.iter().filter(|item| !item.directory).count() as u64;
        begin_transfer(controller.as_deref(), total_bytes, total_files);
        let result: Result<()> = async {
            tokio::fs::create_dir_all(self.local_path(remote_path)?).await?;
            let mut completed = 0;
            for item in items {
                let target = self.local_path(&item.remote)?;
                if item.directory {
                    tokio::fs::create_dir_all(target).await?;
                } else {
                    if let Some(parent) = target.parent() {
                        tokio::fs::create_dir_all(parent).await?;
                    }
                    let partial = partial_path(&target);
                    let item_result: Result<()> = async {
                        copy_local_file(&item.local, &partial, controller.as_deref()).await?;
                        if let Some(controller) = controller.as_deref() {
                            controller.wait_for_transfer_ready().await?;
                        }
                        if tokio::fs::try_exists(&target).await? {
                            tokio::fs::remove_file(&target).await?;
                        }
                        tokio::fs::rename(&partial, &target)
                            .await
                            .map_err(TransportError::from)
                    }
                    .await;
                    if let Err(error) = item_result {
                        let _ = tokio::fs::remove_file(&partial).await;
                        return Err(error);
                    }
                    completed += 1;
                    if let Some(controller) = controller.as_deref() {
                        controller.update_item_progress(completed, total_files);
                    }
                }
            }
            Ok(())
        }
        .await;
        finish_transfer(controller.as_deref(), result)
    }

    async fn download_directory(
        &self,
        remote_path: &Path,
        local_path: &Path,
        controller: Option<Arc<TransferController>>,
    ) -> Result<()> {
        let source = self.local_path(remote_path)?;
        let items = collect_local_tree(&source, local_path).await?;
        let total_bytes = items.iter().map(|item| item.size).sum();
        let total_files = items.iter().filter(|item| !item.directory).count() as u64;
        begin_transfer(controller.as_deref(), total_bytes, total_files);
        let result: Result<()> = async {
            tokio::fs::create_dir_all(local_path).await?;
            let mut completed = 0;
            for item in items {
                if item.directory {
                    tokio::fs::create_dir_all(&item.remote).await?;
                    continue;
                }
                if let Some(parent) = item.remote.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                let partial = partial_path(&item.remote);
                let item_result: Result<()> = async {
                    copy_local_file(&item.local, &partial, controller.as_deref()).await?;
                    if let Some(controller) = controller.as_deref() {
                        controller.wait_for_transfer_ready().await?;
                    }
                    if tokio::fs::try_exists(&item.remote).await? {
                        tokio::fs::remove_file(&item.remote).await?;
                    }
                    tokio::fs::rename(&partial, &item.remote)
                        .await
                        .map_err(TransportError::from)
                }
                .await;
                if let Err(error) = item_result {
                    let _ = tokio::fs::remove_file(&partial).await;
                    return Err(error);
                }
                completed += 1;
                if let Some(controller) = controller.as_deref() {
                    controller.update_item_progress(completed, total_files);
                }
            }
            Ok(())
        }
        .await;
        finish_transfer(controller.as_deref(), result)
    }
}

fn metadata_from_local(metadata: &std::fs::Metadata) -> Metadata {
    let file_type = if metadata.is_dir() {
        FileType::Dir
    } else if metadata.is_file() {
        FileType::File
    } else {
        FileType::Other
    };
    Metadata {
        file_type,
        permissions: FilePermissions::from_mode(0o644),
        size: metadata.len(),
        uid: 0,
        gid: 0,
        accessed: metadata.accessed().ok(),
        modified: metadata.modified().ok(),
    }
}

async fn copy_local_file(
    source: &Path,
    target: &Path,
    controller: Option<&TransferController>,
) -> Result<()> {
    let mut source = tokio::fs::File::open(source).await?;
    let mut target = tokio::fs::File::create(target).await?;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        if let Some(controller) = controller {
            controller.wait_for_transfer_ready().await?;
        }
        let read = source.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        target.write_all(&buffer[..read]).await?;
        if let Some(controller) = controller {
            controller.add_progress(read as u64);
        }
    }
    target.flush().await?;
    Ok(())
}
