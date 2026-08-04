use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use super::transfer::run_transfer_io;
use super::{TransferController, TransferDirection, TransferStatus};

#[tokio::test]
async fn pause_and_resume_unblocks_transfer() {
    let controller = TransferController::new(
        1,
        TransferDirection::Upload,
        PathBuf::from("source"),
        PathBuf::from("target"),
    );
    controller.start(10, 1);
    controller.pause();

    let waiting = {
        let controller = Arc::clone(&controller);
        tokio::spawn(async move { controller.wait_for_transfer_ready().await })
    };
    tokio::task::yield_now().await;
    assert!(!waiting.is_finished());

    controller.resume();
    assert!(waiting.await.expect("等待任务不应 panic").is_ok());
    assert_eq!(controller.snapshot().status, TransferStatus::Running);
}

#[test]
fn pause_before_start_is_preserved() {
    let controller = TransferController::new(
        4,
        TransferDirection::Upload,
        PathBuf::from("source"),
        PathBuf::from("target"),
    );

    controller.pause();
    controller.start(10, 1);

    assert_eq!(controller.snapshot().status, TransferStatus::Paused);
    controller.resume();
    assert_eq!(controller.snapshot().status, TransferStatus::Running);
}

#[tokio::test]
async fn cancel_wakes_waiting_transfer() {
    let controller = TransferController::new(
        2,
        TransferDirection::Download,
        PathBuf::from("source"),
        PathBuf::from("target"),
    );
    controller.start(10, 1);
    controller.pause();

    let waiting = {
        let controller = Arc::clone(&controller);
        tokio::spawn(async move { controller.wait_for_transfer_ready().await })
    };
    controller.cancel();
    let result = waiting.await.expect("等待任务不应 panic");
    assert!(matches!(result, Err(super::TransportError::Cancelled)));
    assert_eq!(controller.snapshot().status, TransferStatus::Cancelled);
}

#[tokio::test]
async fn cancel_wakes_in_flight_io() {
    let controller = TransferController::new(
        5,
        TransferDirection::Upload,
        PathBuf::from("source"),
        PathBuf::from("target"),
    );
    let waiting = {
        let controller = Arc::clone(&controller);
        tokio::spawn(async move {
            run_transfer_io(
                &controller,
                Duration::from_secs(1),
                std::future::pending::<std::result::Result<(), super::TransportError>>(),
            )
            .await
        })
    };

    tokio::task::yield_now().await;
    controller.cancel();
    let result = waiting.await.expect("I/O 等待任务不应 panic");
    assert!(matches!(result, Err(super::TransportError::Cancelled)));
}

#[tokio::test]
async fn in_flight_io_times_out() {
    let controller = TransferController::new(
        6,
        TransferDirection::Download,
        PathBuf::from("source"),
        PathBuf::from("target"),
    );
    let result = run_transfer_io(
        &controller,
        Duration::from_millis(10),
        std::future::pending::<std::result::Result<(), super::TransportError>>(),
    )
    .await;
    assert!(matches!(result, Err(super::TransportError::Timeout)));
}

#[test]
fn retry_resets_progress_and_error() {
    let controller = TransferController::new(
        3,
        TransferDirection::Upload,
        PathBuf::from("source"),
        PathBuf::from("target"),
    );
    controller.start(100, 1);
    controller.update_progress(40, 100);
    controller.fail("网络断开");
    controller.reset_for_retry();

    let snapshot = controller.snapshot();
    assert_eq!(snapshot.status, TransferStatus::Pending);
    assert_eq!(snapshot.transferred_bytes, 0);
    assert_eq!(snapshot.error, None);
}
