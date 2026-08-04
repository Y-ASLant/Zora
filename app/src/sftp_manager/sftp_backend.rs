//! SFTP 后端适配。
//!
//! 真实连接和离线测试后端都实现同一个同步 UI 适配接口，底层操作统一委托给
//! zora_transport 的异步 RemoteFs。

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use async_trait::async_trait;

use super::sftp_ops::{self, ProgressCallback, SftpOpsError};
use super::types::{FileEntry, FileEntryType};

#[async_trait]
pub trait SftpBackend: Send + Sync {
    fn list_dir(&self, path: &Path) -> Result<Vec<FileEntry>, SftpOpsError>;
    fn delete_file(&self, path: &Path) -> Result<(), SftpOpsError>;
    fn delete_dir_recursive(&self, path: &Path) -> Result<(), SftpOpsError>;
    fn create_dir(&self, path: &Path) -> Result<(), SftpOpsError>;
    fn rename(&self, old_path: &Path, new_path: &Path) -> Result<(), SftpOpsError>;
    fn realpath(&self, path: &Path) -> Result<PathBuf, SftpOpsError>;
    fn stat(&self, path: &Path) -> Result<FileEntry, SftpOpsError>;
    fn upload_file(
        &self,
        local_path: &Path,
        remote_path: &Path,
        progress_cb: Option<&ProgressCallback>,
        cancel_flag: Option<&AtomicBool>,
        controller: Option<Arc<zora_transport::TransferController>>,
    ) -> Result<(), SftpOpsError>;
    fn download_file(
        &self,
        remote_path: &Path,
        local_path: &Path,
        progress_cb: Option<&ProgressCallback>,
        cancel_flag: Option<&AtomicBool>,
        controller: Option<Arc<zora_transport::TransferController>>,
    ) -> Result<(), SftpOpsError>;
    fn upload_directory(
        &self,
        local_path: &Path,
        remote_path: &Path,
        controller: Option<Arc<zora_transport::TransferController>>,
    ) -> Result<(), SftpOpsError>;
    fn download_directory(
        &self,
        remote_path: &Path,
        local_path: &Path,
        controller: Option<Arc<zora_transport::TransferController>>,
    ) -> Result<(), SftpOpsError>;
    async fn upload_file_async(
        &self,
        local_path: PathBuf,
        remote_path: PathBuf,
        cancel_flag: Arc<AtomicBool>,
        controller: Arc<zora_transport::TransferController>,
    ) -> Result<(), SftpOpsError>;
    async fn download_file_async(
        &self,
        remote_path: PathBuf,
        local_path: PathBuf,
        cancel_flag: Arc<AtomicBool>,
        controller: Arc<zora_transport::TransferController>,
    ) -> Result<(), SftpOpsError>;
    async fn upload_directory_async(
        &self,
        local_path: PathBuf,
        remote_path: PathBuf,
        cancel_flag: Arc<AtomicBool>,
        controller: Arc<zora_transport::TransferController>,
    ) -> Result<(), SftpOpsError>;
    async fn download_directory_async(
        &self,
        remote_path: PathBuf,
        local_path: PathBuf,
        cancel_flag: Arc<AtomicBool>,
        controller: Arc<zora_transport::TransferController>,
    ) -> Result<(), SftpOpsError>;
}

pub struct LiveSftpBackend {
    sftp: zora_transport::Sftp,
}

impl LiveSftpBackend {
    pub fn new(sftp: zora_transport::Sftp) -> Self {
        Self { sftp }
    }

    pub fn inner(&self) -> &zora_transport::Sftp {
        &self.sftp
    }
}

#[async_trait]
impl SftpBackend for LiveSftpBackend {
    fn list_dir(&self, path: &Path) -> Result<Vec<FileEntry>, SftpOpsError> {
        sftp_ops::list_dir(&self.sftp, path)
    }

    fn delete_file(&self, path: &Path) -> Result<(), SftpOpsError> {
        sftp_ops::delete_file(&self.sftp, path)
    }

    fn delete_dir_recursive(&self, path: &Path) -> Result<(), SftpOpsError> {
        sftp_ops::delete_dir_recursive(&self.sftp, path)
    }

    fn create_dir(&self, path: &Path) -> Result<(), SftpOpsError> {
        sftp_ops::create_dir(&self.sftp, path)
    }

    fn rename(&self, old_path: &Path, new_path: &Path) -> Result<(), SftpOpsError> {
        sftp_ops::rename(&self.sftp, old_path, new_path)
    }

    fn realpath(&self, path: &Path) -> Result<PathBuf, SftpOpsError> {
        sftp_ops::realpath(&self.sftp, path)
    }

    fn stat(&self, path: &Path) -> Result<FileEntry, SftpOpsError> {
        sftp_ops::stat(&self.sftp, path)
    }

    fn upload_file(
        &self,
        local_path: &Path,
        remote_path: &Path,
        progress_cb: Option<&ProgressCallback>,
        cancel_flag: Option<&AtomicBool>,
        controller: Option<Arc<zora_transport::TransferController>>,
    ) -> Result<(), SftpOpsError> {
        static NEVER_CANCEL: AtomicBool = AtomicBool::new(false);
        sftp_ops::upload_file_streaming_with_controller(
            &self.sftp,
            local_path,
            remote_path,
            progress_cb,
            cancel_flag.unwrap_or(&NEVER_CANCEL),
            controller.unwrap_or_else(|| {
                zora_transport::TransferController::new(
                    0,
                    zora_transport::TransferDirection::Upload,
                    local_path.to_path_buf(),
                    remote_path.to_path_buf(),
                )
            }),
        )
    }

    fn download_file(
        &self,
        remote_path: &Path,
        local_path: &Path,
        progress_cb: Option<&ProgressCallback>,
        cancel_flag: Option<&AtomicBool>,
        controller: Option<Arc<zora_transport::TransferController>>,
    ) -> Result<(), SftpOpsError> {
        static NEVER_CANCEL: AtomicBool = AtomicBool::new(false);
        sftp_ops::download_file_streaming_with_controller(
            &self.sftp,
            remote_path,
            local_path,
            progress_cb,
            cancel_flag.unwrap_or(&NEVER_CANCEL),
            controller.unwrap_or_else(|| {
                zora_transport::TransferController::new(
                    0,
                    zora_transport::TransferDirection::Download,
                    remote_path.to_path_buf(),
                    local_path.to_path_buf(),
                )
            }),
        )
    }

    fn upload_directory(
        &self,
        local_path: &Path,
        remote_path: &Path,
        controller: Option<Arc<zora_transport::TransferController>>,
    ) -> Result<(), SftpOpsError> {
        sftp_ops::block_on_transport(zora_transport::RemoteFs::upload_directory(
            &self.sftp,
            local_path,
            remote_path,
            controller,
        ))?;
        Ok(())
    }

    fn download_directory(
        &self,
        remote_path: &Path,
        local_path: &Path,
        controller: Option<Arc<zora_transport::TransferController>>,
    ) -> Result<(), SftpOpsError> {
        sftp_ops::block_on_transport(zora_transport::RemoteFs::download_directory(
            &self.sftp,
            remote_path,
            local_path,
            controller,
        ))?;
        Ok(())
    }

    async fn upload_file_async(
        &self,
        local_path: PathBuf,
        remote_path: PathBuf,
        cancel_flag: Arc<AtomicBool>,
        controller: Arc<zora_transport::TransferController>,
    ) -> Result<(), SftpOpsError> {
        sftp_ops::upload_file_streaming_async(
            self.sftp.clone(),
            local_path,
            remote_path,
            cancel_flag,
            controller,
        )
        .await
    }

    async fn download_file_async(
        &self,
        remote_path: PathBuf,
        local_path: PathBuf,
        cancel_flag: Arc<AtomicBool>,
        controller: Arc<zora_transport::TransferController>,
    ) -> Result<(), SftpOpsError> {
        sftp_ops::download_file_streaming_async(
            self.sftp.clone(),
            remote_path,
            local_path,
            cancel_flag,
            controller,
        )
        .await
    }

    async fn upload_directory_async(
        &self,
        local_path: PathBuf,
        remote_path: PathBuf,
        cancel_flag: Arc<AtomicBool>,
        controller: Arc<zora_transport::TransferController>,
    ) -> Result<(), SftpOpsError> {
        sftp_ops::upload_directory_async(
            self.sftp.clone(),
            local_path,
            remote_path,
            cancel_flag,
            controller,
        )
        .await
    }

    async fn download_directory_async(
        &self,
        remote_path: PathBuf,
        local_path: PathBuf,
        cancel_flag: Arc<AtomicBool>,
        controller: Arc<zora_transport::TransferController>,
    ) -> Result<(), SftpOpsError> {
        sftp_ops::download_directory_async(
            self.sftp.clone(),
            remote_path,
            local_path,
            cancel_flag,
            controller,
        )
        .await
    }
}

pub struct InMemorySftpBackend {
    root: PathBuf,
    remote_fs: zora_transport::LocalRemoteFs,
}

impl InMemorySftpBackend {
    pub fn new(root: PathBuf) -> Self {
        Self {
            remote_fs: zora_transport::LocalRemoteFs::new(root.clone()),
            root,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn entry_from_transport(entry: zora_transport::DirEntry) -> FileEntry {
        entry_from_transport(entry.path, entry.name, entry.metadata)
    }
}

#[async_trait]
impl SftpBackend for InMemorySftpBackend {
    fn list_dir(&self, path: &Path) -> Result<Vec<FileEntry>, SftpOpsError> {
        Ok(
            sftp_ops::block_on_transport(zora_transport::RemoteFs::list_dir(
                &self.remote_fs,
                path,
            ))?
            .into_iter()
            .map(Self::entry_from_transport)
            .collect(),
        )
    }

    fn delete_file(&self, path: &Path) -> Result<(), SftpOpsError> {
        sftp_ops::block_on_transport(zora_transport::RemoteFs::remove_file(&self.remote_fs, path))?;
        Ok(())
    }

    fn delete_dir_recursive(&self, path: &Path) -> Result<(), SftpOpsError> {
        sftp_ops::block_on_transport(zora_transport::RemoteFs::remove_dir_all(
            &self.remote_fs,
            path,
        ))?;
        Ok(())
    }

    fn create_dir(&self, path: &Path) -> Result<(), SftpOpsError> {
        sftp_ops::block_on_transport(zora_transport::RemoteFs::mkdir(&self.remote_fs, path))?;
        Ok(())
    }

    fn rename(&self, old_path: &Path, new_path: &Path) -> Result<(), SftpOpsError> {
        sftp_ops::block_on_transport(zora_transport::RemoteFs::rename(
            &self.remote_fs,
            old_path,
            new_path,
            zora_transport::RenameOptions::default(),
        ))?;
        Ok(())
    }

    fn realpath(&self, path: &Path) -> Result<PathBuf, SftpOpsError> {
        Ok(sftp_ops::block_on_transport(
            zora_transport::RemoteFs::realpath(&self.remote_fs, path),
        )?)
    }

    fn stat(&self, path: &Path) -> Result<FileEntry, SftpOpsError> {
        let metadata =
            sftp_ops::block_on_transport(zora_transport::RemoteFs::stat(&self.remote_fs, path))?;
        Ok(entry_from_transport(
            path.to_path_buf(),
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            metadata,
        ))
    }

    fn upload_file(
        &self,
        local_path: &Path,
        remote_path: &Path,
        progress_cb: Option<&ProgressCallback>,
        cancel_flag: Option<&AtomicBool>,
        _controller: Option<Arc<zora_transport::TransferController>>,
    ) -> Result<(), SftpOpsError> {
        if cancel_flag.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::SeqCst)) {
            return Err(SftpOpsError::Cancelled);
        }
        let total = std::fs::metadata(local_path)?.len();
        sftp_ops::block_on_transport(zora_transport::RemoteFs::upload_file(
            &self.remote_fs,
            local_path,
            remote_path,
            None,
        ))?;
        if let Some(progress_cb) = progress_cb {
            progress_cb(total, total);
        }
        Ok(())
    }

    fn download_file(
        &self,
        remote_path: &Path,
        local_path: &Path,
        progress_cb: Option<&ProgressCallback>,
        cancel_flag: Option<&AtomicBool>,
        _controller: Option<Arc<zora_transport::TransferController>>,
    ) -> Result<(), SftpOpsError> {
        if cancel_flag.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::SeqCst)) {
            return Err(SftpOpsError::Cancelled);
        }
        let total = sftp_ops::block_on_transport(zora_transport::RemoteFs::stat(
            &self.remote_fs,
            remote_path,
        ))?
        .size;
        sftp_ops::block_on_transport(zora_transport::RemoteFs::download_file(
            &self.remote_fs,
            remote_path,
            local_path,
            None,
        ))?;
        if let Some(progress_cb) = progress_cb {
            progress_cb(total, total);
        }
        Ok(())
    }

    fn upload_directory(
        &self,
        local_path: &Path,
        remote_path: &Path,
        controller: Option<Arc<zora_transport::TransferController>>,
    ) -> Result<(), SftpOpsError> {
        sftp_ops::block_on_transport(zora_transport::RemoteFs::upload_directory(
            &self.remote_fs,
            local_path,
            remote_path,
            controller,
        ))?;
        Ok(())
    }

    fn download_directory(
        &self,
        remote_path: &Path,
        local_path: &Path,
        controller: Option<Arc<zora_transport::TransferController>>,
    ) -> Result<(), SftpOpsError> {
        sftp_ops::block_on_transport(zora_transport::RemoteFs::download_directory(
            &self.remote_fs,
            remote_path,
            local_path,
            controller,
        ))?;
        Ok(())
    }

    async fn upload_file_async(
        &self,
        local_path: PathBuf,
        remote_path: PathBuf,
        cancel_flag: Arc<AtomicBool>,
        controller: Arc<zora_transport::TransferController>,
    ) -> Result<(), SftpOpsError> {
        if cancel_flag.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(SftpOpsError::Cancelled);
        }
        zora_transport::RemoteFs::upload_file(
            &self.remote_fs,
            &local_path,
            &remote_path,
            Some(controller),
        )
        .await?;
        Ok(())
    }

    async fn download_file_async(
        &self,
        remote_path: PathBuf,
        local_path: PathBuf,
        cancel_flag: Arc<AtomicBool>,
        controller: Arc<zora_transport::TransferController>,
    ) -> Result<(), SftpOpsError> {
        if cancel_flag.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(SftpOpsError::Cancelled);
        }
        zora_transport::RemoteFs::download_file(
            &self.remote_fs,
            &remote_path,
            &local_path,
            Some(controller),
        )
        .await?;
        Ok(())
    }

    async fn upload_directory_async(
        &self,
        local_path: PathBuf,
        remote_path: PathBuf,
        cancel_flag: Arc<AtomicBool>,
        controller: Arc<zora_transport::TransferController>,
    ) -> Result<(), SftpOpsError> {
        if cancel_flag.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(SftpOpsError::Cancelled);
        }
        zora_transport::RemoteFs::upload_directory(
            &self.remote_fs,
            &local_path,
            &remote_path,
            Some(controller),
        )
        .await?;
        Ok(())
    }

    async fn download_directory_async(
        &self,
        remote_path: PathBuf,
        local_path: PathBuf,
        cancel_flag: Arc<AtomicBool>,
        controller: Arc<zora_transport::TransferController>,
    ) -> Result<(), SftpOpsError> {
        if cancel_flag.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(SftpOpsError::Cancelled);
        }
        zora_transport::RemoteFs::download_directory(
            &self.remote_fs,
            &remote_path,
            &local_path,
            Some(controller),
        )
        .await?;
        Ok(())
    }
}

fn entry_from_transport(
    path: PathBuf,
    name: String,
    metadata: zora_transport::Metadata,
) -> FileEntry {
    let file_type = match metadata.file_type {
        zora_transport::FileType::Dir => FileEntryType::Directory,
        zora_transport::FileType::File => FileEntryType::File,
        zora_transport::FileType::Symlink => FileEntryType::Symlink,
        zora_transport::FileType::Other => FileEntryType::Other,
    };
    let modified = metadata.modified.map(|time| {
        let datetime: chrono::DateTime<chrono::Local> = time.into();
        datetime.format("%Y-%m-%d %H:%M").to_string()
    });
    FileEntry {
        name,
        path,
        file_type,
        size: metadata.size,
        modified,
        permissions: Some(metadata.permissions.to_string()),
    }
}

impl InMemorySftpBackend {
    pub fn into_backend(self) -> Arc<dyn SftpBackend> {
        Arc::new(self)
    }
}
