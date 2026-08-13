use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use warp_core::HostId;
use warp_util::content_version::ContentVersion;
use warp_util::standardized_path::StandardizedPath;

/// Identifies a file on a remote host.
///
/// Pairs a [`HostId`] (to deduplicate across multiple SSH sessions to the
/// same host) with the server-side [`StandardizedPath`].
///
/// 实现 `Serialize`/`Deserialize` 仅为让其能作为 `CodeSource` 的字段编译通过
/// (`CodeSource` 整体派生 serde);远端文件 pane 实际不持久化。
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RemotePath {
    pub host_id: HostId,
    pub path: StandardizedPath,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SftpPath {
    pub node_id: String,
    pub path: PathBuf,
    #[serde(default)]
    pub display_identity: Option<String>,
}

impl PartialEq for SftpPath {
    fn eq(&self, other: &Self) -> bool {
        self.node_id == other.node_id && self.path == other.path
    }
}

impl Eq for SftpPath {}

impl Hash for SftpPath {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.node_id.hash(state);
        self.path.hash(state);
    }
}

impl RemotePath {
    #[cfg_attr(not(feature = "local_tty"), allow(dead_code))]
    pub fn new(host_id: HostId, path: StandardizedPath) -> Self {
        Self { host_id, path }
    }

    /// 远端文件名(取路径最后一段),用作 tab / pane header 标题。
    pub fn file_name(&self) -> &str {
        self.path
            .as_str()
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| self.path.as_str())
    }
}

/// Uniquely identifies where a buffer's content lives.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum BufferLocation {
    /// File on the local filesystem.
    Local(PathBuf),
    /// File on a remote host, identified by host + path.
    Remote(RemotePath),
    /// File on an SSH/SFTP connection managed by the local SSH manager.
    Sftp(SftpPath),
}

impl BufferLocation {
    /// 本地路径(仅 `Local` 变体有);远端文件返回 `None`。
    pub fn local_path(&self) -> Option<&std::path::Path> {
        match self {
            BufferLocation::Local(path) => Some(path.as_path()),
            BufferLocation::Remote(_) | BufferLocation::Sftp(_) => None,
        }
    }

    /// 用于 tab / header 显示的文件名。
    pub fn display_name(&self) -> String {
        match self {
            BufferLocation::Local(path) => path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| path.display().to_string()),
            BufferLocation::Remote(remote) => remote.file_name().to_string(),
            BufferLocation::Sftp(sftp) => sftp
                .path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| sftp.path.display().to_string()),
        }
    }

    pub fn source_label(&self) -> Option<String> {
        match self {
            BufferLocation::Local(_) => None,
            BufferLocation::Remote(remote) => Some(format!(
                "{}:{}",
                remote.host_id.as_str(),
                remote.path.as_str()
            )),
            BufferLocation::Sftp(sftp) => Some(format!(
                "{}:{}",
                sftp.display_identity
                    .as_deref()
                    .unwrap_or(sftp.node_id.as_str()),
                sftp.path.display()
            )),
        }
    }

    /// 用于语言识别(后缀)的路径。远端文件用其远端路径构造一个
    /// `PathBuf`(只取后缀,不做文件系统访问)。
    pub fn language_path(&self) -> PathBuf {
        match self {
            BufferLocation::Local(path) => path.clone(),
            BufferLocation::Remote(remote) => PathBuf::from(remote.path.as_str()),
            BufferLocation::Sftp(sftp) => sftp.path.clone(),
        }
    }
}

/// Tracks sync state between client and server for a single remote buffer.
///
/// Uses a version vector with two components:
/// - `server_version`: bumped by the server when the file changes on disk.
/// - `client_version`: bumped by the client when the user edits the buffer.
///
/// Conflict detection:
/// - Server pushes `{S_new, C_expected}`. Client checks `C_expected == local client_version`.
///   Match → accept. Mismatch → conflict.
/// - Client sends `{S_expected, C_new}`. Server checks `S_expected == local server_version`.
///   Match → accept. Mismatch → reject (server pushes its current state).
///
/// Both fields use `ContentVersion` internally. At the wire boundary (proto
/// encode/decode), convert via `ContentVersion::as_u64()` and
/// `ContentVersion::from_raw()`.
#[derive(Clone, Debug)]
pub struct SyncClock {
    /// Last version acknowledged from the server (file-watcher side).
    pub server_version: ContentVersion,
    /// Last version acknowledged from the client (user-edit side).
    pub client_version: ContentVersion,
}

impl SyncClock {
    #[cfg_attr(not(feature = "local_fs"), allow(dead_code))]
    pub fn new() -> Self {
        Self {
            server_version: ContentVersion::from_raw(0),
            client_version: ContentVersion::from_raw(0),
        }
    }

    /// Reconstruct a `SyncClock` from wire values (proto deserialization).
    /// 用 `from_wire_u64` 饱和而不是 `as usize`,避免 32-bit 平台上隐式截断。
    pub fn from_wire(server_version: u64, client_version: u64) -> Self {
        Self {
            server_version: ContentVersion::from_wire_u64(server_version),
            client_version: ContentVersion::from_wire_u64(client_version),
        }
    }

    /// Bump the server version after a file-watcher change.
    #[cfg_attr(not(feature = "local_fs"), allow(dead_code))]
    pub fn bump_server(&mut self) -> ContentVersion {
        self.server_version = ContentVersion::new();
        self.server_version
    }

    /// Check whether a server push's expected client version matches our local state.
    pub fn server_push_matches(&self, expected_client_version: ContentVersion) -> bool {
        self.client_version == expected_client_version
    }

    /// Check whether a client edit's expected server version matches our local state.
    #[cfg_attr(not(feature = "local_fs"), allow(dead_code))]
    pub fn client_edit_matches(&self, expected_server_version: ContentVersion) -> bool {
        self.server_version == expected_server_version
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use warp_util::file_type::is_markdown_file;

    fn remote(path: &str) -> BufferLocation {
        BufferLocation::Remote(RemotePath::new(
            HostId::new("test-host".to_string()),
            StandardizedPath::try_new(path).unwrap(),
        ))
    }

    #[test]
    fn remote_markdown_detected_via_language_path() {
        // 远端文件没有本地路径,Markdown 识别必须走 `language_path()`(只取后缀)。
        assert!(is_markdown_file(
            remote("/home/user/notes/README.md").language_path()
        ));
        assert!(is_markdown_file(
            remote("/home/user/doc.markdown").language_path()
        ));
        assert!(is_markdown_file(remote("/srv/CHANGELOG").language_path()));
        assert!(!is_markdown_file(
            remote("/home/user/src/main.rs").language_path()
        ));
        assert!(!is_markdown_file(
            remote("/home/user/data.json").language_path()
        ));
    }
}
