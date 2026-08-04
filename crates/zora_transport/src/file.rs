use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::error::{Result, sftp_error};
use crate::types::{Metadata, OpenOptions, WriteMode};

/// 异步远程文件句柄。
pub struct RemoteFile {
    handle: russh_sftp::client::fs::File,
}

impl RemoteFile {
    pub(crate) fn new(handle: russh_sftp::client::fs::File) -> Self {
        Self { handle }
    }

    pub async fn read_to_end(&mut self, buffer: &mut Vec<u8>) -> Result<u64> {
        Ok(self.handle.read_to_end(buffer).await? as u64)
    }

    pub async fn read(&mut self, buffer: &mut [u8]) -> Result<usize> {
        Ok(self.handle.read(buffer).await?)
    }

    pub async fn write_all(&mut self, buffer: &[u8]) -> Result<()> {
        self.handle.write_all(buffer).await?;
        Ok(())
    }

    pub async fn flush(&mut self) -> Result<()> {
        self.handle.flush().await?;
        Ok(())
    }

    pub async fn close(self) -> Result<()> {
        self.handle.close().await?;
        Ok(())
    }

    pub async fn stat(&self) -> Result<Metadata> {
        let metadata = self.handle.metadata().await.map_err(sftp_error)?;
        Ok(Metadata::from_sftp(&metadata))
    }
}

pub(crate) fn to_open_flags(options: &OpenOptions) -> russh_sftp::protocol::OpenFlags {
    let mut flags = russh_sftp::protocol::OpenFlags::empty();
    if options.read {
        flags |= russh_sftp::protocol::OpenFlags::READ;
    }
    if options.write.is_some() {
        flags |= russh_sftp::protocol::OpenFlags::WRITE;
    }
    if options.create {
        flags |= russh_sftp::protocol::OpenFlags::CREATE;
    }
    if options.truncate {
        flags |= russh_sftp::protocol::OpenFlags::TRUNCATE;
    }
    if matches!(options.write, Some(WriteMode::Append)) {
        flags |= russh_sftp::protocol::OpenFlags::APPEND;
    }
    flags
}
