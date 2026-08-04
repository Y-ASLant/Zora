use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::error::{Result, TransportError};

pub type TransferId = u64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferDirection {
    Upload,
    Download,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferStatus {
    Pending,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug)]
pub struct TransferSnapshot {
    pub id: TransferId,
    pub source: PathBuf,
    pub target: PathBuf,
    pub direction: TransferDirection,
    pub status: TransferStatus,
    pub total_bytes: u64,
    pub transferred_bytes: u64,
    pub total_files: u64,
    pub completed_files: u64,
    pub speed_bytes_per_second: u64,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub enum TransferEvent {
    Started(TransferSnapshot),
    Progress(TransferSnapshot),
    Paused(TransferSnapshot),
    Resumed(TransferSnapshot),
    Completed(TransferSnapshot),
    Cancelled(TransferSnapshot),
    Failed(TransferSnapshot),
}

pub type TransferListener = Arc<dyn Fn(TransferEvent) + Send + Sync>;

struct TransferState {
    snapshot: TransferSnapshot,
    started_at: Option<Instant>,
}

/// 可暂停、恢复、取消和重试的单个传输控制器。
pub struct TransferController {
    state: Mutex<TransferState>,
    notify: tokio::sync::Notify,
    listener: Option<TransferListener>,
}

impl std::fmt::Debug for TransferController {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TransferController")
            .field("snapshot", &self.snapshot())
            .finish()
    }
}

impl TransferController {
    pub fn new(
        id: TransferId,
        direction: TransferDirection,
        source: PathBuf,
        target: PathBuf,
    ) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(TransferState {
                snapshot: TransferSnapshot {
                    id,
                    source,
                    target,
                    direction,
                    status: TransferStatus::Pending,
                    total_bytes: 0,
                    transferred_bytes: 0,
                    total_files: 1,
                    completed_files: 0,
                    speed_bytes_per_second: 0,
                    error: None,
                },
                started_at: None,
            }),
            notify: tokio::sync::Notify::new(),
            listener: None,
        })
    }

    pub fn with_listener(self: Arc<Self>, listener: TransferListener) -> Arc<Self> {
        let mut controller = self;
        let state = Arc::get_mut(&mut controller).expect("传输控制器必须在启动前设置监听器");
        state.listener = Some(listener);
        controller
    }

    pub fn snapshot(&self) -> TransferSnapshot {
        lock(&self.state).snapshot.clone()
    }

    pub fn start(&self, total_bytes: u64, total_files: u64) {
        let snapshot = {
            let mut state = lock(&self.state);
            if matches!(
                state.snapshot.status,
                TransferStatus::Completed | TransferStatus::Failed | TransferStatus::Cancelled
            ) {
                return;
            }
            let was_paused = state.snapshot.status == TransferStatus::Paused;
            state.snapshot.total_bytes = total_bytes;
            state.snapshot.total_files = total_files;
            if !was_paused {
                state.snapshot.status = TransferStatus::Running;
            }
            state.started_at = Some(Instant::now());
            state.snapshot.clone()
        };
        self.emit(TransferEvent::Started(snapshot));
        self.notify.notify_waiters();
    }

    pub async fn wait_for_transfer_ready(&self) -> Result<()> {
        loop {
            let notified = self.notify.notified();
            let status = lock(&self.state).snapshot.status;
            match status {
                TransferStatus::Pending | TransferStatus::Running => return Ok(()),
                TransferStatus::Paused => notified.await,
                TransferStatus::Cancelled => return Err(TransportError::Cancelled),
                TransferStatus::Completed => {
                    return Err(TransportError::General("传输已经完成".to_string()));
                }
                TransferStatus::Failed => {
                    let error = lock(&self.state)
                        .snapshot
                        .error
                        .clone()
                        .unwrap_or_else(|| "传输失败".to_string());
                    return Err(TransportError::General(error));
                }
            }
        }
    }

    pub fn pause(&self) {
        let snapshot = {
            let mut state = lock(&self.state);
            if !matches!(
                state.snapshot.status,
                TransferStatus::Pending | TransferStatus::Running
            ) {
                return;
            }
            state.snapshot.status = TransferStatus::Paused;
            state.snapshot.clone()
        };
        self.emit(TransferEvent::Paused(snapshot));
    }

    pub fn resume(&self) {
        let snapshot = {
            let mut state = lock(&self.state);
            if !matches!(state.snapshot.status, TransferStatus::Paused) {
                return;
            }
            state.snapshot.status = TransferStatus::Running;
            state.snapshot.clone()
        };
        self.emit(TransferEvent::Resumed(snapshot));
        self.notify.notify_waiters();
    }

    pub fn cancel(&self) {
        let snapshot = {
            let mut state = lock(&self.state);
            if matches!(
                state.snapshot.status,
                TransferStatus::Completed | TransferStatus::Cancelled
            ) {
                return;
            }
            state.snapshot.status = TransferStatus::Cancelled;
            state.snapshot.clone()
        };
        self.emit(TransferEvent::Cancelled(snapshot));
        self.notify.notify_waiters();
    }

    pub fn update_progress(&self, transferred_bytes: u64, total_bytes: u64) {
        let snapshot = {
            let mut state = lock(&self.state);
            state.snapshot.transferred_bytes = transferred_bytes;
            state.snapshot.total_bytes = total_bytes;
            state.snapshot.speed_bytes_per_second = state
                .started_at
                .and_then(|started| {
                    let seconds = started.elapsed().as_secs_f64();
                    (seconds > 0.0).then(|| (transferred_bytes as f64 / seconds) as u64)
                })
                .unwrap_or_default();
            state.snapshot.clone()
        };
        self.emit(TransferEvent::Progress(snapshot));
    }

    pub fn add_progress(&self, bytes: u64) {
        let (total, transferred) = {
            let state = lock(&self.state);
            (
                state.snapshot.total_bytes,
                state.snapshot.transferred_bytes.saturating_add(bytes),
            )
        };
        self.update_progress(transferred, total);
    }

    pub fn update_item_progress(&self, completed_files: u64, total_files: u64) {
        let snapshot = {
            let mut state = lock(&self.state);
            state.snapshot.completed_files = completed_files;
            state.snapshot.total_files = total_files;
            state.snapshot.clone()
        };
        self.emit(TransferEvent::Progress(snapshot));
    }

    pub fn complete(&self) {
        let snapshot = {
            let mut state = lock(&self.state);
            state.snapshot.status = TransferStatus::Completed;
            state.snapshot.transferred_bytes = state.snapshot.total_bytes;
            state.snapshot.completed_files = state.snapshot.total_files;
            state.snapshot.clone()
        };
        self.emit(TransferEvent::Completed(snapshot));
        self.notify.notify_waiters();
    }

    pub fn fail(&self, error: impl Into<String>) {
        let snapshot = {
            let mut state = lock(&self.state);
            state.snapshot.status = TransferStatus::Failed;
            state.snapshot.error = Some(error.into());
            state.snapshot.clone()
        };
        self.emit(TransferEvent::Failed(snapshot));
        self.notify.notify_waiters();
    }

    pub fn reset_for_retry(&self) {
        let mut state = lock(&self.state);
        state.snapshot.status = TransferStatus::Pending;
        state.snapshot.transferred_bytes = 0;
        state.snapshot.completed_files = 0;
        state.snapshot.speed_bytes_per_second = 0;
        state.snapshot.error = None;
        state.started_at = None;
    }

    fn emit(&self, event: TransferEvent) {
        if let Some(listener) = &self.listener {
            listener(event);
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[derive(Debug, Default)]
pub struct TransferRegistry {
    next_id: AtomicU64,
    controllers: Mutex<HashMap<TransferId, Arc<TransferController>>>,
}

impl TransferRegistry {
    pub fn new() -> Self {
        Self {
            next_id: AtomicU64::new(1),
            controllers: Mutex::new(HashMap::new()),
        }
    }

    pub fn create(
        &self,
        direction: TransferDirection,
        source: PathBuf,
        target: PathBuf,
    ) -> Arc<TransferController> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let controller = TransferController::new(id, direction, source, target);
        lock(&self.controllers).insert(id, Arc::clone(&controller));
        controller
    }

    pub fn get(&self, id: TransferId) -> Option<Arc<TransferController>> {
        lock(&self.controllers).get(&id).cloned()
    }

    pub fn remove(&self, id: TransferId) -> Option<Arc<TransferController>> {
        lock(&self.controllers).remove(&id)
    }

    pub fn snapshots(&self) -> Vec<TransferSnapshot> {
        lock(&self.controllers)
            .values()
            .map(|controller| controller.snapshot())
            .collect()
    }
}
