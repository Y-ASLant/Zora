use std::path::{Path, PathBuf};

use crate::error::{Result, sftp_error};
use crate::types::{DirEntry, FileType, Metadata};

pub(crate) async fn read_dir(
    sftp: &russh_sftp::client::SftpSession,
    path: &str,
) -> Result<Vec<DirEntry>> {
    let mut entries = Vec::new();
    let mut remote_entries = sftp.read_dir(path.to_owned()).await.map_err(sftp_error)?;
    while let Some(entry) = remote_entries.next() {
        let name = entry.file_name();
        if name == "." || name == ".." {
            continue;
        }
        let entry_path = entry.path();
        entries.push(DirEntry {
            name,
            path: PathBuf::from(entry_path),
            metadata: Metadata::from_sftp(&entry.metadata()),
        });
    }
    entries.sort_by(|left, right| {
        let left_is_dir = left.metadata.file_type == FileType::Dir;
        let right_is_dir = right.metadata.file_type == FileType::Dir;
        right_is_dir
            .cmp(&left_is_dir)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    Ok(entries)
}

pub(crate) fn remote_path(path: &Path) -> Result<String> {
    let path = path.to_string_lossy().replace('\\', "/");
    if path.contains('\0') {
        return Err(crate::TransportError::InvalidPath(path));
    }
    if path.split('/').any(|component| component == "..") {
        return Err(crate::TransportError::InvalidPath(path));
    }
    Ok(path)
}
