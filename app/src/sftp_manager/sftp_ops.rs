//! SFTP 操作适配层。
//!
//! 传输实现全部来自 zora_transport，这里仅负责把服务器凭据和 UI 类型接起来。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use warp_ssh_manager::secrets::SshSecretStore;
use warp_ssh_manager::types::{AuthType, ResolvedSshAuth, SshServerInfo};
use zora_transport::{
    AuthMethod, FileType, RemoteFs, ServerKeyPolicy, Sftp, SftpSession, TransferController,
    TransferDirection,
};

use super::types::{FileEntry, FileEntryType};

#[derive(Debug)]
pub enum SftpOpsError {
    Connection(String),
    Operation(String),
    LocalIo(String),
    NoCredentials(String),
    Cancelled,
}

impl std::fmt::Display for SftpOpsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connection(message) => write!(formatter, "连接错误: {message}"),
            Self::Operation(message) => write!(formatter, "操作错误: {message}"),
            Self::LocalIo(message) => write!(formatter, "本地 IO 错误: {message}"),
            Self::NoCredentials(message) => write!(formatter, "未找到凭据: {message}"),
            Self::Cancelled => formatter.write_str("传输已取消"),
        }
    }
}

impl From<zora_transport::TransportError> for SftpOpsError {
    fn from(error: zora_transport::TransportError) -> Self {
        match error {
            zora_transport::TransportError::Io(error) => Self::LocalIo(error.to_string()),
            zora_transport::TransportError::Ssh(error)
            | zora_transport::TransportError::Sftp(error)
            | zora_transport::TransportError::General(error)
            | zora_transport::TransportError::Unsupported(error)
            | zora_transport::TransportError::InvalidPath(error) => Self::Operation(error),
            zora_transport::TransportError::ConnectionFailed(error)
            | zora_transport::TransportError::AuthenticationFailed(error) => {
                Self::Connection(error)
            }
            zora_transport::TransportError::Timeout => Self::Connection("操作超时".to_string()),
            zora_transport::TransportError::NoSuchFile(path)
            | zora_transport::TransportError::PermissionDenied(path) => {
                Self::Operation(path.display().to_string())
            }
            zora_transport::TransportError::Cancelled => Self::Cancelled,
        }
    }
}

impl From<std::io::Error> for SftpOpsError {
    fn from(error: std::io::Error) -> Self {
        Self::LocalIo(error.to_string())
    }
}

pub type ProgressCallback = Box<dyn Fn(u64, u64) + Send>;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

pub fn connect_from_server(
    server: &SshServerInfo,
    secret_store: &dyn SshSecretStore,
) -> Result<SftpSession, SftpOpsError> {
    let resolved_auth = resolve_sftp_auth(server)?;
    let auth = build_auth_method(server, &resolved_auth, secret_store)?;
    warpui::r#async::block_on(SftpSession::connect_with_policy(
        &server.host,
        server.port,
        &resolved_auth.username,
        auth,
        Some(CONNECT_TIMEOUT),
        ServerKeyPolicy::AcceptAny,
    ))
    .map_err(|error| SftpOpsError::Connection(error.to_string()))
}

fn resolve_sftp_auth(server: &SshServerInfo) -> Result<ResolvedSshAuth, SftpOpsError> {
    warp_ssh_manager::with_conn(|connection| {
        Ok(warp_ssh_manager::SshRepository::resolve_server_auth(
            connection, server,
        )?)
    })
    .map_err(|error| SftpOpsError::NoCredentials(format!("解析认证失败: {error}")))
}

pub fn list_dir(sftp: &Sftp, path: &Path) -> Result<Vec<FileEntry>, SftpOpsError> {
    let entries = warpui::r#async::block_on(sftp.read_dir(path))?;
    Ok(entries.into_iter().map(file_entry_from_transport).collect())
}

pub fn delete_file(sftp: &Sftp, path: &Path) -> Result<(), SftpOpsError> {
    warpui::r#async::block_on(sftp.remove_file(path))?;
    Ok(())
}

pub fn delete_dir_recursive(sftp: &Sftp, path: &Path) -> Result<(), SftpOpsError> {
    warpui::r#async::block_on(RemoteFs::remove_dir_all(sftp, path))?;
    Ok(())
}

pub fn create_dir(sftp: &Sftp, path: &Path) -> Result<(), SftpOpsError> {
    warpui::r#async::block_on(sftp.create_dir(path))?;
    Ok(())
}

pub fn rename(sftp: &Sftp, old_path: &Path, new_path: &Path) -> Result<(), SftpOpsError> {
    warpui::r#async::block_on(sftp.rename(
        old_path,
        new_path,
        zora_transport::RenameOptions::default(),
    ))?;
    Ok(())
}

pub fn realpath(sftp: &Sftp, path: &Path) -> Result<PathBuf, SftpOpsError> {
    Ok(warpui::r#async::block_on(sftp.realpath(path))?)
}

pub fn stat(sftp: &Sftp, path: &Path) -> Result<FileEntry, SftpOpsError> {
    Ok(file_entry_from_transport_path(
        path.to_path_buf(),
        warpui::r#async::block_on(sftp.stat(path))?,
    ))
}

pub fn upload_file_streaming(
    sftp: &Sftp,
    local_path: &Path,
    remote_path: &Path,
    progress_cb: Option<&ProgressCallback>,
    cancel_flag: &AtomicBool,
) -> Result<(), SftpOpsError> {
    upload_file_streaming_with_controller(
        sftp,
        local_path,
        remote_path,
        progress_cb,
        cancel_flag,
        TransferController::new(
            1,
            TransferDirection::Upload,
            local_path.to_path_buf(),
            remote_path.to_path_buf(),
        ),
    )
}

pub fn upload_file_streaming_with_controller(
    sftp: &Sftp,
    local_path: &Path,
    remote_path: &Path,
    progress_cb: Option<&ProgressCallback>,
    cancel_flag: &AtomicBool,
    controller: Arc<TransferController>,
) -> Result<(), SftpOpsError> {
    run_transfer(
        sftp,
        local_path,
        remote_path,
        TransferDirection::Upload,
        progress_cb,
        cancel_flag,
        controller,
    )
}

pub fn download_file_streaming(
    sftp: &Sftp,
    remote_path: &Path,
    local_path: &Path,
    progress_cb: Option<&ProgressCallback>,
    cancel_flag: &AtomicBool,
) -> Result<(), SftpOpsError> {
    download_file_streaming_with_controller(
        sftp,
        remote_path,
        local_path,
        progress_cb,
        cancel_flag,
        TransferController::new(
            1,
            TransferDirection::Download,
            remote_path.to_path_buf(),
            local_path.to_path_buf(),
        ),
    )
}

pub fn download_file_streaming_with_controller(
    sftp: &Sftp,
    remote_path: &Path,
    local_path: &Path,
    progress_cb: Option<&ProgressCallback>,
    cancel_flag: &AtomicBool,
    controller: Arc<TransferController>,
) -> Result<(), SftpOpsError> {
    run_transfer(
        sftp,
        remote_path,
        local_path,
        TransferDirection::Download,
        progress_cb,
        cancel_flag,
        controller,
    )
}

fn run_transfer(
    sftp: &Sftp,
    source: &Path,
    target: &Path,
    direction: TransferDirection,
    progress_cb: Option<&ProgressCallback>,
    cancel_flag: &AtomicBool,
    controller: Arc<TransferController>,
) -> Result<(), SftpOpsError> {
    let result = warpui::r#async::block_on(async {
        let operation = async {
            match direction {
                TransferDirection::Upload => {
                    RemoteFs::upload_file(sftp, source, target, Some(controller.clone())).await
                }
                TransferDirection::Download => {
                    RemoteFs::download_file(sftp, source, target, Some(controller.clone())).await
                }
            }
        };
        tokio::pin!(operation);
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        loop {
            tokio::select! {
                result = &mut operation => break result,
                _ = tick.tick() => {
                    if cancel_flag.load(Ordering::SeqCst) {
                        controller.cancel();
                    }
                    report_progress(progress_cb, &controller);
                }
            }
        }
    });
    report_progress(progress_cb, &controller);
    result.map_err(Into::into)
}

fn report_progress(progress_cb: Option<&ProgressCallback>, controller: &TransferController) {
    if let Some(progress_cb) = progress_cb {
        let snapshot = controller.snapshot();
        progress_cb(snapshot.transferred_bytes, snapshot.total_bytes);
    }
}

fn file_entry_from_transport(entry: zora_transport::DirEntry) -> FileEntry {
    file_entry_from_transport_path(entry.path, entry.metadata).with_name(entry.name)
}

fn file_entry_from_transport_path(path: PathBuf, metadata: zora_transport::Metadata) -> FileEntry {
    let file_type = match metadata.file_type {
        FileType::Dir => FileEntryType::Directory,
        FileType::File => FileEntryType::File,
        FileType::Symlink => FileEntryType::Symlink,
        FileType::Other => FileEntryType::Other,
    };
    let modified = metadata.modified.map(|time| {
        let datetime: chrono::DateTime<chrono::Local> = time.into();
        datetime.format("%Y-%m-%d %H:%M").to_string()
    });
    let permissions = Some(metadata.permissions.to_string());
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    FileEntry {
        name,
        path,
        file_type,
        size: metadata.size,
        modified,
        permissions,
    }
}

trait WithName {
    fn with_name(self, name: String) -> Self;
}

impl WithName for FileEntry {
    fn with_name(mut self, name: String) -> Self {
        self.name = name;
        self
    }
}

fn build_auth_method(
    server: &SshServerInfo,
    resolved_auth: &ResolvedSshAuth,
    secret_store: &dyn SshSecretStore,
) -> Result<AuthMethod, SftpOpsError> {
    match resolved_auth.auth_type {
        AuthType::Password | AuthType::OneKey => {
            let password = secret_store
                .get(&resolved_auth.secret_lookup_id, resolved_auth.secret_kind)
                .map_err(|error| SftpOpsError::NoCredentials(format!("读取密码失败: {error}")))?
                .ok_or_else(|| {
                    SftpOpsError::NoCredentials(format!("服务器 {} 未存储密码", server.host))
                })?;
            Ok(AuthMethod::Password {
                password: password.to_string(),
            })
        }
        AuthType::Key => {
            let key_path = resolved_auth.key_path.as_ref().ok_or_else(|| {
                SftpOpsError::NoCredentials("密钥认证但未指定密钥路径".to_string())
            })?;
            let passphrase = secret_store
                .get(&resolved_auth.secret_lookup_id, resolved_auth.secret_kind)
                .ok()
                .flatten()
                .map(|value| value.to_string());
            Ok(AuthMethod::PublicKey {
                key_path: PathBuf::from(shellexpand_path(key_path)),
                passphrase,
            })
        }
    }
}

fn shellexpand_path(path: &str) -> String {
    if path.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            return format!("{}/{suffix}", home.display(), suffix = &path[2..]);
        }
    }
    path.to_string()
}

pub(crate) fn bool_to_rwx(read: bool, write: bool, exec: bool) -> String {
    let mut result = String::with_capacity(3);
    result.push(if read { 'r' } else { '-' });
    result.push(if write { 'w' } else { '-' });
    result.push(if exec { 'x' } else { '-' });
    result
}

pub(crate) fn normalize_remote_path(path: &PathBuf) -> PathBuf {
    PathBuf::from(path.to_string_lossy().replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_is_stable() {
        assert_eq!(
            SftpOpsError::Connection("refused".to_string()).to_string(),
            "连接错误: refused"
        );
        assert_eq!(
            SftpOpsError::Operation("not found".to_string()).to_string(),
            "操作错误: not found"
        );
        assert_eq!(SftpOpsError::Cancelled.to_string(), "传输已取消");
    }

    #[test]
    fn permissions_are_rendered_as_rwx() {
        assert_eq!(bool_to_rwx(true, false, true), "r-x");
        assert_eq!(bool_to_rwx(false, true, false), "-w-");
    }

    #[test]
    fn remote_paths_use_posix_separators() {
        assert_eq!(
            normalize_remote_path(&PathBuf::from(r"\var\tmp\a.txt")),
            PathBuf::from("/var/tmp/a.txt")
        );
    }
}
