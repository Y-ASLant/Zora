use std::path::Path;
use std::sync::Arc;

use super::{LocalRemoteFs, RemoteFs, TransferController, TransferDirection, TransferStatus};

#[tokio::test]
async fn local_backend_transfers_files_and_directories() {
    let local_source = tempfile::tempdir().expect("创建源目录失败");
    let remote_root = tempfile::tempdir().expect("创建远程根目录失败");
    let local_target = tempfile::tempdir().expect("创建目标目录失败");
    let nested = local_source.path().join("目录");
    tokio::fs::create_dir(&nested)
        .await
        .expect("创建嵌套目录失败");
    tokio::fs::write(nested.join("文件.txt"), b"zora transport")
        .await
        .expect("写入测试文件失败");

    let remote = LocalRemoteFs::new(remote_root.path().to_path_buf());
    let controller = TransferController::new(
        10,
        TransferDirection::Upload,
        local_source.path().to_path_buf(),
        "/bundle".into(),
    );
    remote
        .upload_directory(
            local_source.path(),
            Path::new("/bundle"),
            Some(Arc::clone(&controller)),
        )
        .await
        .expect("目录上传失败");
    assert_eq!(controller.snapshot().status, TransferStatus::Completed);

    remote
        .download_directory(
            Path::new("/bundle"),
            &local_target.path().join("bundle"),
            None,
        )
        .await
        .expect("目录下载失败");
    let content = tokio::fs::read(local_target.path().join("bundle/目录/文件.txt"))
        .await
        .expect("读取下载文件失败");
    assert_eq!(content, b"zora transport");
}

#[tokio::test]
async fn local_backend_rejects_parent_traversal() {
    let remote_root = tempfile::tempdir().expect("创建远程根目录失败");
    let remote = LocalRemoteFs::new(remote_root.path().to_path_buf());
    let result = remote.list_dir(Path::new("/../outside")).await;
    assert!(matches!(result, Err(super::TransportError::InvalidPath(_))));
}
