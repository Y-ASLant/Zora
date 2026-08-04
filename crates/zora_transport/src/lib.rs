//! Zora 远程传输核心。
//!
//! 该 crate 只负责远程文件系统、SSH/SFTP 会话和可控制的文件传输，
//! 不依赖 WarpUI，因此可以被 UI、测试和其它远程功能复用。

mod dir;
mod error;
mod file;
mod remote_fs;
mod session;
mod sftp;
mod transfer;
mod types;

pub use error::{Result, TransportError};
pub use remote_fs::{LocalRemoteFs, RemoteFs};
pub use session::{AuthMethod, CommandOutput, ServerKeyPolicy, SftpSession};
pub use sftp::Sftp;
pub use transfer::{
    TransferController, TransferDirection, TransferEvent, TransferId, TransferListener,
    TransferRegistry, TransferSnapshot, TransferStatus,
};
pub use types::{
    DirEntry, FilePermissions, FileType, Metadata, OpenFileType, OpenOptions, RenameOptions,
    WriteMode,
};

pub use file::RemoteFile;

#[cfg(test)]
#[path = "transfer_tests.rs"]
mod transfer_tests;

#[cfg(test)]
#[path = "remote_fs_tests.rs"]
mod remote_fs_tests;
