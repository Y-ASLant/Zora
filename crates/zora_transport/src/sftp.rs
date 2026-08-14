use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::dir::{read_dir, remote_path};
use crate::error::{Result, TransportError, sftp_error};
use crate::file::{RemoteFile, to_open_flags};
use crate::session::CommandOutput;
use crate::transfer::{TransferController, run_transfer_io};
use crate::types::{DirEntry, Metadata, OpenOptions, RenameOptions};

const TRANSFER_IO_TIMEOUT: Duration = Duration::from_secs(30);

/// SFTP 高级操作封装。
#[derive(Clone)]
pub struct Sftp {
    inner: Arc<russh_sftp::client::SftpSession>,
}

impl std::fmt::Debug for Sftp {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Sftp").finish_non_exhaustive()
    }
}

impl Sftp {
    pub(crate) fn new(inner: russh_sftp::client::SftpSession) -> Self {
        Self {
            inner: Arc::new(inner),
        }
    }

    pub async fn read_dir(&self, path: &Path) -> Result<Vec<DirEntry>> {
        read_dir(&self.inner, &remote_path(path)?).await
    }

    pub async fn stat(&self, path: &Path) -> Result<Metadata> {
        Ok(Metadata::from_sftp(
            &self
                .inner
                .metadata(remote_path(path)?)
                .await
                .map_err(sftp_error)?,
        ))
    }

    pub async fn lstat(&self, path: &Path) -> Result<Metadata> {
        Ok(Metadata::from_sftp(
            &self
                .inner
                .symlink_metadata(remote_path(path)?)
                .await
                .map_err(sftp_error)?,
        ))
    }

    pub async fn realpath(&self, path: &Path) -> Result<PathBuf> {
        Ok(PathBuf::from(
            self.inner
                .canonicalize(remote_path(path)?)
                .await
                .map_err(sftp_error)?,
        ))
    }

    pub async fn create_dir(&self, path: &Path) -> Result<()> {
        self.inner
            .create_dir(remote_path(path)?)
            .await
            .map_err(sftp_error)
    }

    pub async fn remove_file(&self, path: &Path) -> Result<()> {
        self.inner
            .remove_file(remote_path(path)?)
            .await
            .map_err(sftp_error)
    }

    pub async fn remove_dir(&self, path: &Path) -> Result<()> {
        self.inner
            .remove_dir(remote_path(path)?)
            .await
            .map_err(sftp_error)
    }

    pub async fn rename(
        &self,
        old_path: &Path,
        new_path: &Path,
        options: RenameOptions,
    ) -> Result<()> {
        if options.overwrite && self.try_exists(new_path).await? {
            let metadata = self.lstat(new_path).await?;
            if metadata.file_type == crate::types::FileType::Dir {
                self.remove_dir(new_path).await?;
            } else {
                self.remove_file(new_path).await?;
            }
        }
        self.inner
            .rename(remote_path(old_path)?, remote_path(new_path)?)
            .await
            .map_err(sftp_error)
    }

    pub async fn try_exists(&self, path: &Path) -> Result<bool> {
        self.inner
            .try_exists(remote_path(path)?)
            .await
            .map_err(sftp_error)
    }

    pub async fn open(&self, path: &Path, options: &OpenOptions) -> Result<RemoteFile> {
        let handle = self
            .inner
            .open_with_flags_and_attributes(
                remote_path(path)?,
                to_open_flags(options),
                options
                    .mode
                    .map(|mode| russh_sftp::protocol::FileAttributes {
                        permissions: Some(mode),
                        ..Default::default()
                    })
                    .unwrap_or_default(),
            )
            .await
            .map_err(sftp_error)?;
        Ok(RemoteFile::new(handle))
    }

    pub async fn read(&self, path: &Path) -> Result<Vec<u8>> {
        let mut file = self.open(path, &OpenOptions::read()).await?;
        let mut bytes = Vec::new();
        let mut buffer = vec![0_u8; 64 * 1024];
        loop {
            let read = file.read(&mut buffer).await?;
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..read]);
        }
        file.close().await?;
        Ok(bytes)
    }

    pub async fn write(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        let mut file = self.open(path, &OpenOptions::write()).await?;
        file.write_all(bytes).await?;
        file.flush().await?;
        file.close().await
    }

    pub async fn canonicalize(&self, path: PathBuf) -> Result<PathBuf> {
        self.realpath(&path).await
    }

    pub(crate) async fn upload_stream(
        &self,
        local_path: &Path,
        remote_path: &Path,
        controller: Option<&TransferController>,
    ) -> Result<u64> {
        if let Some(controller) = controller {
            controller.wait_for_transfer_ready().await?;
        }
        let mut local = tokio::fs::File::open(local_path).await?;
        let mut remote =
            transfer_io(controller, self.open(remote_path, &OpenOptions::write())).await?;
        let mut transferred = 0;
        let mut buffer = vec![0_u8; 64 * 1024];
        loop {
            if let Some(controller) = controller {
                controller.wait_for_transfer_ready().await?;
            }
            let read = local.read(&mut buffer).await?;
            if read == 0 {
                break;
            }
            transfer_io(controller, remote.write_all(&buffer[..read])).await?;
            transferred += read as u64;
            if let Some(controller) = controller {
                controller.add_progress(read as u64);
            }
        }
        transfer_io(controller, remote.flush()).await?;
        transfer_io(controller, remote.close()).await?;
        Ok(transferred)
    }

    pub(crate) async fn download_stream(
        &self,
        remote_path: &Path,
        local_path: &Path,
        controller: Option<&TransferController>,
    ) -> Result<u64> {
        if let Some(controller) = controller {
            controller.wait_for_transfer_ready().await?;
        }
        let mut remote =
            transfer_io(controller, self.open(remote_path, &OpenOptions::read())).await?;
        let mut local = tokio::fs::File::create(local_path).await?;
        let mut transferred = 0;
        let mut buffer = vec![0_u8; 64 * 1024];
        loop {
            if let Some(controller) = controller {
                controller.wait_for_transfer_ready().await?;
            }
            let read = transfer_io(controller, remote.read(&mut buffer)).await?;
            if read == 0 {
                break;
            }
            local.write_all(&buffer[..read]).await?;
            transferred += read as u64;
            if let Some(controller) = controller {
                controller.add_progress(read as u64);
            }
        }
        local.flush().await?;
        transfer_io(controller, remote.close()).await?;
        Ok(transferred)
    }

    pub(crate) async fn execute(&self, command: &str) -> Result<CommandOutput> {
        let _ = command;
        Err(crate::TransportError::Unsupported(
            "SFTP 会话不保留 SSH 命令通道".to_string(),
        ))
    }
}

async fn transfer_io<T, F>(controller: Option<&TransferController>, future: F) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    match controller {
        Some(controller) => run_transfer_io(controller, TRANSFER_IO_TIMEOUT, future).await,
        None => tokio::time::timeout(TRANSFER_IO_TIMEOUT, future)
            .await
            .map_err(|_| TransportError::Timeout)?,
    }
}
