use std::path::PathBuf;
use std::time::SystemTime;

/// 远程条目类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    Dir,
    File,
    Symlink,
    Other,
}

impl FileType {
    pub fn from_mode(mode: u32) -> Self {
        match mode & 0o170000 {
            0o040000 => Self::Dir,
            0o100000 => Self::File,
            0o120000 => Self::Symlink,
            _ => Self::Other,
        }
    }
}

/// Unix 风格文件权限。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilePermissions {
    pub owner_read: bool,
    pub owner_write: bool,
    pub owner_exec: bool,
    pub group_read: bool,
    pub group_write: bool,
    pub group_exec: bool,
    pub other_read: bool,
    pub other_write: bool,
    pub other_exec: bool,
}

impl FilePermissions {
    pub fn from_mode(mode: u32) -> Self {
        Self {
            owner_read: mode & 0o400 != 0,
            owner_write: mode & 0o200 != 0,
            owner_exec: mode & 0o100 != 0,
            group_read: mode & 0o040 != 0,
            group_write: mode & 0o020 != 0,
            group_exec: mode & 0o010 != 0,
            other_read: mode & 0o004 != 0,
            other_write: mode & 0o002 != 0,
            other_exec: mode & 0o001 != 0,
        }
    }

    pub fn mode(self) -> u32 {
        let mut mode = 0;
        if self.owner_read {
            mode |= 0o400;
        }
        if self.owner_write {
            mode |= 0o200;
        }
        if self.owner_exec {
            mode |= 0o100;
        }
        if self.group_read {
            mode |= 0o040;
        }
        if self.group_write {
            mode |= 0o020;
        }
        if self.group_exec {
            mode |= 0o010;
        }
        if self.other_read {
            mode |= 0o004;
        }
        if self.other_write {
            mode |= 0o002;
        }
        if self.other_exec {
            mode |= 0o001;
        }
        mode
    }
}

impl std::fmt::Display for FilePermissions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let permissions = [
            (self.owner_read, 'r'),
            (self.owner_write, 'w'),
            (self.owner_exec, 'x'),
            (self.group_read, 'r'),
            (self.group_write, 'w'),
            (self.group_exec, 'x'),
            (self.other_read, 'r'),
            (self.other_write, 'w'),
            (self.other_exec, 'x'),
        ];
        for (enabled, character) in permissions {
            std::fmt::Write::write_char(formatter, if enabled { character } else { '-' })?;
        }
        Ok(())
    }
}

/// 远程文件元数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metadata {
    pub file_type: FileType,
    pub permissions: FilePermissions,
    pub size: u64,
    pub uid: u32,
    pub gid: u32,
    pub accessed: Option<SystemTime>,
    pub modified: Option<SystemTime>,
}

impl Metadata {
    pub fn from_sftp(stat: &russh_sftp::protocol::FileAttributes) -> Self {
        let mode = stat.permissions.unwrap_or(0);
        Self {
            file_type: FileType::from_mode(mode),
            permissions: FilePermissions::from_mode(mode),
            size: stat.size.unwrap_or_default(),
            uid: stat.uid.unwrap_or_default(),
            gid: stat.gid.unwrap_or_default(),
            accessed: stat.atime.map(|seconds| {
                SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(u64::from(seconds))
            }),
            modified: stat.mtime.map(|seconds| {
                SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(u64::from(seconds))
            }),
        }
    }
}

impl Metadata {
    pub fn to_sftp_attributes(&self) -> russh_sftp::protocol::FileAttributes {
        russh_sftp::protocol::FileAttributes {
            size: Some(self.size),
            uid: Some(self.uid),
            gid: Some(self.gid),
            permissions: Some(self.permissions.mode()),
            atime: self.accessed.and_then(unix_seconds),
            mtime: self.modified.and_then(unix_seconds),
            ..Default::default()
        }
    }
}

fn unix_seconds(time: SystemTime) -> Option<u32> {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .and_then(|duration| u32::try_from(duration.as_secs()).ok())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteMode {
    Write,
    Append,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenFileType {
    File,
    Dir,
}

#[derive(Debug, Clone)]
pub struct OpenOptions {
    pub read: bool,
    pub write: Option<WriteMode>,
    pub create: bool,
    pub truncate: bool,
    pub mode: Option<u32>,
    pub file_type: OpenFileType,
}

impl OpenOptions {
    pub fn read() -> Self {
        Self {
            read: true,
            write: None,
            create: false,
            truncate: false,
            mode: None,
            file_type: OpenFileType::File,
        }
    }

    pub fn write() -> Self {
        Self {
            read: false,
            write: Some(WriteMode::Write),
            create: true,
            truncate: true,
            mode: Some(0o644),
            file_type: OpenFileType::File,
        }
    }

    pub fn append() -> Self {
        Self {
            read: false,
            write: Some(WriteMode::Append),
            create: true,
            truncate: false,
            mode: Some(0o644),
            file_type: OpenFileType::File,
        }
    }

    pub fn create_new() -> Self {
        Self {
            read: false,
            write: Some(WriteMode::Write),
            create: true,
            truncate: false,
            mode: Some(0o644),
            file_type: OpenFileType::File,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RenameOptions {
    pub overwrite: bool,
    pub atomic: bool,
    pub native: bool,
}

#[derive(Debug, Clone)]
pub struct DirEntry {
    pub name: String,
    pub path: PathBuf,
    pub metadata: Metadata,
}
