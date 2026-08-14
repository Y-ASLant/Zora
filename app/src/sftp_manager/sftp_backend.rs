//! SFTP 后端适配。
//!
//! 真实连接和离线测试后端都实现同一个同步 UI 适配接口，底层操作统一委托给
//! zora_transport 的异步 RemoteFs。

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use async_trait::async_trait;

use super::sftp_ops::{self, ProgressCallback, SftpOpsError};
use super::types::{
    FileEntry, FileEntryType, RemoteFileBytes, RemoteFileVersion, RemoteFileWriteMode,
    RemoteFileWriteResult,
};

#[async_trait]
pub trait SftpBackend: Send + Sync {
    fn list_dir(&self, path: &Path) -> Result<Vec<FileEntry>, SftpOpsError>;
    fn delete_file(&self, path: &Path) -> Result<(), SftpOpsError>;
    fn delete_dir_recursive(&self, path: &Path) -> Result<(), SftpOpsError>;
    fn create_dir(&self, path: &Path) -> Result<(), SftpOpsError>;
    fn rename(&self, old_path: &Path, new_path: &Path) -> Result<(), SftpOpsError>;
    fn realpath(&self, path: &Path) -> Result<PathBuf, SftpOpsError>;
    fn stat(&self, path: &Path) -> Result<FileEntry, SftpOpsError>;
    fn file_version(&self, path: &Path) -> Result<RemoteFileVersion, SftpOpsError>;
    fn read_file(&self, path: &Path, max_bytes: u64) -> Result<RemoteFileBytes, SftpOpsError>;
    fn read_file_range(&self, path: &Path, offset: u64, len: u64) -> Result<Vec<u8>, SftpOpsError>;
    fn write_file(
        &self,
        path: &Path,
        bytes: &[u8],
        expected: Option<RemoteFileVersion>,
        mode: RemoteFileWriteMode,
    ) -> Result<RemoteFileWriteResult, SftpOpsError>;
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

    fn file_version(&self, path: &Path) -> Result<RemoteFileVersion, SftpOpsError> {
        sftp_ops::log_diagnostic_operation("file_version", path.display().to_string(), || {
            let metadata = sftp_ops::block_on_transport(self.sftp.stat(path))?;
            validate_regular_file(&metadata)?;
            Ok(RemoteFileVersion::from_metadata(&metadata))
        })
    }

    fn read_file(&self, path: &Path, max_bytes: u64) -> Result<RemoteFileBytes, SftpOpsError> {
        read_remote_file(&self.sftp, path, max_bytes)
    }

    fn read_file_range(&self, path: &Path, offset: u64, len: u64) -> Result<Vec<u8>, SftpOpsError> {
        read_remote_file_range(&self.sftp, path, offset, len)
    }

    fn write_file(
        &self,
        path: &Path,
        bytes: &[u8],
        expected: Option<RemoteFileVersion>,
        mode: RemoteFileWriteMode,
    ) -> Result<RemoteFileWriteResult, SftpOpsError> {
        write_remote_file(&self.sftp, path, bytes, expected, mode)
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
        sftp_ops::log_diagnostic_operation(
            "upload_directory",
            format!("{} -> {}", local_path.display(), remote_path.display()),
            || {
                sftp_ops::block_on_transport(zora_transport::RemoteFs::upload_directory(
                    &self.sftp,
                    local_path,
                    remote_path,
                    controller,
                ))?;
                Ok(())
            },
        )
    }

    fn download_directory(
        &self,
        remote_path: &Path,
        local_path: &Path,
        controller: Option<Arc<zora_transport::TransferController>>,
    ) -> Result<(), SftpOpsError> {
        sftp_ops::log_diagnostic_operation(
            "download_directory",
            format!("{} -> {}", remote_path.display(), local_path.display()),
            || {
                sftp_ops::block_on_transport(zora_transport::RemoteFs::download_directory(
                    &self.sftp,
                    remote_path,
                    local_path,
                    controller,
                ))?;
                Ok(())
            },
        )
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

    fn local_path(&self, remote: &Path) -> Result<PathBuf, SftpOpsError> {
        let remote = remote.to_string_lossy().replace('\\', "/");
        let mut path = self.root.clone();
        for component in remote.split('/') {
            if component.is_empty() || component == "." {
                continue;
            }
            if component == ".." {
                return Err(SftpOpsError::Operation(format!("非法远程路径: {}", remote)));
            }
            path.push(component);
        }
        Ok(path)
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
        let local_path = self.local_path(path)?;
        let metadata = metadata_from_local(&std::fs::symlink_metadata(local_path)?);
        Ok(entry_from_transport(
            path.to_path_buf(),
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            metadata,
        ))
    }

    fn file_version(&self, path: &Path) -> Result<RemoteFileVersion, SftpOpsError> {
        let local_path = self.local_path(path)?;
        let metadata = metadata_from_local(&std::fs::symlink_metadata(local_path)?);
        validate_regular_file(&metadata)?;
        Ok(RemoteFileVersion::from_metadata(&metadata))
    }

    fn read_file(&self, path: &Path, max_bytes: u64) -> Result<RemoteFileBytes, SftpOpsError> {
        let local_path = self.local_path(path)?;
        let metadata = metadata_from_local(&std::fs::symlink_metadata(&local_path)?);
        validate_readable_file(&metadata, max_bytes)?;
        let bytes = std::fs::read(local_path)?;
        if bytes.len() as u64 > max_bytes {
            return Err(SftpOpsError::FileTooLarge {
                size: bytes.len() as u64,
                max: max_bytes,
            });
        }
        Ok(remote_file_bytes(bytes, metadata))
    }

    fn read_file_range(&self, path: &Path, offset: u64, len: u64) -> Result<Vec<u8>, SftpOpsError> {
        let local_path = self.local_path(path)?;
        let metadata = metadata_from_local(&std::fs::symlink_metadata(&local_path)?);
        validate_regular_file(&metadata)?;
        let bytes = std::fs::read(local_path)?;
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(bytes.len());
        let requested_end = offset.saturating_add(len);
        let end = usize::try_from(requested_end)
            .unwrap_or(usize::MAX)
            .min(bytes.len());
        Ok(bytes[start..end].to_vec())
    }

    fn write_file(
        &self,
        path: &Path,
        bytes: &[u8],
        expected: Option<RemoteFileVersion>,
        mode: RemoteFileWriteMode,
    ) -> Result<RemoteFileWriteResult, SftpOpsError> {
        let local_path = self.local_path(path)?;
        match std::fs::symlink_metadata(&local_path) {
            Ok(metadata) => {
                let current = RemoteFileVersion::from_metadata(&metadata_from_local(&metadata));
                match mode {
                    RemoteFileWriteMode::Create => {
                        return Ok(RemoteFileWriteResult::Conflict { current });
                    }
                    RemoteFileWriteMode::Normal
                        if expected.is_some_and(|version| version != current) =>
                    {
                        return Ok(RemoteFileWriteResult::Conflict { current });
                    }
                    RemoteFileWriteMode::Normal | RemoteFileWriteMode::Force => {}
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }

        if let Some(parent) = local_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&local_path, bytes)?;
        let metadata = metadata_from_local(&std::fs::symlink_metadata(&local_path)?);
        Ok(RemoteFileWriteResult::Saved {
            version: RemoteFileVersion::from_metadata(&metadata),
        })
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
    let file_type = file_type_from_transport(metadata.file_type);
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

fn file_type_from_transport(file_type: zora_transport::FileType) -> FileEntryType {
    match file_type {
        zora_transport::FileType::Dir => FileEntryType::Directory,
        zora_transport::FileType::File => FileEntryType::File,
        zora_transport::FileType::Symlink => FileEntryType::Symlink,
        zora_transport::FileType::Other => FileEntryType::Other,
    }
}

fn validate_regular_file(metadata: &zora_transport::Metadata) -> Result<(), SftpOpsError> {
    if metadata.file_type != zora_transport::FileType::File {
        return Err(SftpOpsError::Operation(
            "远程路径不是普通文件，无法作为文件打开".to_string(),
        ));
    }
    Ok(())
}

fn validate_readable_file(
    metadata: &zora_transport::Metadata,
    max_bytes: u64,
) -> Result<(), SftpOpsError> {
    validate_regular_file(metadata)?;
    if metadata.size > max_bytes {
        return Err(SftpOpsError::FileTooLarge {
            size: metadata.size,
            max: max_bytes,
        });
    }
    Ok(())
}

fn remote_file_bytes(bytes: Vec<u8>, metadata: zora_transport::Metadata) -> RemoteFileBytes {
    RemoteFileBytes {
        bytes,
        version: RemoteFileVersion::from_metadata(&metadata),
        file_type: file_type_from_transport(metadata.file_type),
    }
}

fn metadata_from_local(metadata: &std::fs::Metadata) -> zora_transport::Metadata {
    let local_file_type = metadata.file_type();
    let file_type = if local_file_type.is_dir() {
        zora_transport::FileType::Dir
    } else if local_file_type.is_file() {
        zora_transport::FileType::File
    } else if local_file_type.is_symlink() {
        zora_transport::FileType::Symlink
    } else {
        zora_transport::FileType::Other
    };
    zora_transport::Metadata {
        file_type,
        permissions: zora_transport::FilePermissions::from_mode(0o644),
        size: metadata.len(),
        uid: 0,
        gid: 0,
        accessed: metadata.accessed().ok(),
        modified: metadata.modified().ok(),
    }
}

fn read_remote_file(
    sftp: &zora_transport::Sftp,
    path: &Path,
    max_bytes: u64,
) -> Result<RemoteFileBytes, SftpOpsError> {
    sftp_ops::log_diagnostic_operation(
        "read_file",
        format!("path={} max_bytes={max_bytes}", path.display()),
        || {
            let metadata = sftp_ops::block_on_transport(sftp.stat(path))?;
            validate_readable_file(&metadata, max_bytes)?;
            let bytes = sftp_ops::block_on_transport(sftp.read_limited(path, max_bytes))?;
            if bytes.len() as u64 > max_bytes {
                return Err(SftpOpsError::FileTooLarge {
                    size: bytes.len() as u64,
                    max: max_bytes,
                });
            }
            Ok(remote_file_bytes(bytes, metadata))
        },
    )
}

fn read_remote_file_range(
    sftp: &zora_transport::Sftp,
    path: &Path,
    offset: u64,
    len: u64,
) -> Result<Vec<u8>, SftpOpsError> {
    sftp_ops::log_diagnostic_operation(
        "read_file_range",
        format!("path={} offset={offset} len={len}", path.display()),
        || {
            let metadata = sftp_ops::block_on_transport(sftp.stat(path))?;
            validate_regular_file(&metadata)?;
            let max_bytes = offset.saturating_add(len);
            let bytes = read_remote_file_prefix(sftp, path, max_bytes)?;
            let start = usize::try_from(offset)
                .unwrap_or(usize::MAX)
                .min(bytes.len());
            let end = usize::try_from(max_bytes)
                .unwrap_or(usize::MAX)
                .min(bytes.len());
            Ok(bytes[start..end].to_vec())
        },
    )
}

fn read_remote_file_prefix(
    sftp: &zora_transport::Sftp,
    path: &Path,
    max_bytes: u64,
) -> Result<Vec<u8>, SftpOpsError> {
    sftp_ops::log_diagnostic_operation(
        "read_file_prefix",
        format!("path={} max_bytes={max_bytes}", path.display()),
        || {
            sftp_ops::block_on_transport(async {
                let mut file = sftp
                    .open(path, &zora_transport::OpenOptions::read())
                    .await?;
                let mut bytes = Vec::new();
                let mut buffer = vec![0_u8; 64 * 1024];
                while bytes.len() < max_bytes as usize {
                    let remaining = max_bytes as usize - bytes.len();
                    let read_len = remaining.min(buffer.len());
                    let read = file.read(&mut buffer[..read_len]).await?;
                    if read == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&buffer[..read]);
                }
                file.close().await?;
                Ok::<_, zora_transport::TransportError>(bytes)
            })
            .map_err(Into::into)
        },
    )
}

fn write_remote_file(
    sftp: &zora_transport::Sftp,
    path: &Path,
    bytes: &[u8],
    expected: Option<RemoteFileVersion>,
    mode: RemoteFileWriteMode,
) -> Result<RemoteFileWriteResult, SftpOpsError> {
    sftp_ops::log_diagnostic_operation(
        "write_file",
        format!(
            "path={} bytes={} mode={mode:?} expected={}",
            path.display(),
            bytes.len(),
            expected.is_some()
        ),
        || {
            match sftp_ops::block_on_transport(sftp.stat(path)) {
                Ok(current_metadata) => {
                    let current = RemoteFileVersion::from_metadata(&current_metadata);
                    match mode {
                        RemoteFileWriteMode::Create => {
                            return Ok(RemoteFileWriteResult::Conflict { current });
                        }
                        RemoteFileWriteMode::Normal
                            if expected.is_some_and(|version| version != current) =>
                        {
                            return Ok(RemoteFileWriteResult::Conflict { current });
                        }
                        RemoteFileWriteMode::Normal | RemoteFileWriteMode::Force => {}
                    }
                }
                Err(_) => {}
            }

            sftp_ops::block_on_transport(sftp.write(path, bytes))?;
            let metadata = sftp_ops::block_on_transport(sftp.stat(path))?;
            Ok(RemoteFileWriteResult::Saved {
                version: RemoteFileVersion::from_metadata(&metadata),
            })
        },
    )
}

impl InMemorySftpBackend {
    pub fn into_backend(self) -> Arc<dyn SftpBackend> {
        Arc::new(self)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn backend_with_file(path: &str, content: &[u8]) -> (tempfile::TempDir, InMemorySftpBackend) {
        let temp = tempfile::tempdir().expect("创建临时目录失败");
        let local_path = temp.path().join(path);
        if let Some(parent) = local_path.parent() {
            std::fs::create_dir_all(parent).expect("创建父目录失败");
        }
        std::fs::write(local_path, content).expect("写入测试文件失败");
        let backend = InMemorySftpBackend::new(temp.path().to_path_buf());
        (temp, backend)
    }

    #[test]
    fn read_file_returns_bytes_and_version() {
        let (_temp, backend) = backend_with_file("readme.txt", b"hello");

        let result = backend
            .read_file(Path::new("/readme.txt"), 8)
            .expect("读取远程文件失败");

        assert_eq!(result.bytes, b"hello");
        assert_eq!(result.version.size, 5);
        assert_eq!(result.file_type, FileEntryType::File);
    }

    #[test]
    fn read_file_rejects_files_larger_than_limit() {
        let (_temp, backend) = backend_with_file("large.log", b"0123456789");

        let error = backend
            .read_file(Path::new("/large.log"), 4)
            .expect_err("超过限制的文件应失败");

        assert!(matches!(
            error,
            SftpOpsError::FileTooLarge { size: 10, max: 4 }
        ));
    }

    #[test]
    fn read_file_range_returns_requested_slice() {
        let (_temp, backend) = backend_with_file("data.txt", b"abcdef");

        let result = backend
            .read_file_range(Path::new("/data.txt"), 2, 3)
            .expect("读取远程文件范围失败");

        assert_eq!(result, b"cde");
    }

    #[test]
    fn write_file_reports_conflict_when_remote_version_changed() {
        let (temp, backend) = backend_with_file("conflict.txt", b"base");
        let original = backend
            .read_file(Path::new("/conflict.txt"), 16)
            .expect("读取远程文件失败")
            .version;
        std::fs::write(temp.path().join("conflict.txt"), b"changed").expect("修改测试文件失败");

        let result = backend
            .write_file(
                Path::new("/conflict.txt"),
                b"mine",
                Some(original),
                RemoteFileWriteMode::Normal,
            )
            .expect("冲突检测不应返回 IO 错误");

        assert!(matches!(result, RemoteFileWriteResult::Conflict { .. }));
        assert_eq!(
            std::fs::read(temp.path().join("conflict.txt")).expect("读取测试文件失败"),
            b"changed"
        );
    }

    #[test]
    fn write_file_force_overwrites_conflict() {
        let (temp, backend) = backend_with_file("force.txt", b"base");
        let original = backend
            .read_file(Path::new("/force.txt"), 16)
            .expect("读取远程文件失败")
            .version;
        std::fs::write(temp.path().join("force.txt"), b"changed").expect("修改测试文件失败");

        let result = backend
            .write_file(
                Path::new("/force.txt"),
                b"mine",
                Some(original),
                RemoteFileWriteMode::Force,
            )
            .expect("强制写入失败");

        assert!(matches!(result, RemoteFileWriteResult::Saved { .. }));
        assert_eq!(
            std::fs::read(temp.path().join("force.txt")).expect("读取测试文件失败"),
            b"mine"
        );
    }

    #[test]
    fn write_file_create_writes_new_file() {
        let temp = tempfile::tempdir().expect("创建临时目录失败");
        let backend = InMemorySftpBackend::new(temp.path().to_path_buf());

        let result = backend
            .write_file(
                Path::new("/new.txt"),
                b"new content",
                None,
                RemoteFileWriteMode::Create,
            )
            .expect("创建远程文件失败");

        assert!(matches!(result, RemoteFileWriteResult::Saved { .. }));
        assert_eq!(
            std::fs::read(temp.path().join("new.txt")).expect("读取测试文件失败"),
            b"new content"
        );
    }

    #[test]
    fn write_file_create_refuses_to_overwrite_existing_file() {
        let (temp, backend) = backend_with_file("exists.txt", b"base");

        let result = backend
            .write_file(
                Path::new("/exists.txt"),
                b"new content",
                None,
                RemoteFileWriteMode::Create,
            )
            .expect("create-only 冲突不应返回 IO 错误");

        assert!(matches!(result, RemoteFileWriteResult::Conflict { .. }));
        assert_eq!(
            std::fs::read(temp.path().join("exists.txt")).expect("读取测试文件失败"),
            b"base"
        );
    }
}
