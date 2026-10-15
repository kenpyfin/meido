use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tokio::sync::Mutex;
use tracing::{info, warn};

type BoxTaskFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;
type RunCancel = Arc<AtomicBool>;
type RunRegistryValue = (QueueLaneKey, RunCancel);
type RunRegistry = HashMap<String, RunRegistryValue>;
/// Called when a running queue task is aborted (hard timeout or panic) so callers
/// can deliver a user-visible notice and mark `run_finished`.
pub type QueueHardAbortHook =
    Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;
const QUEUED_TASK_HARD_TIMEOUT: Duration = Duration::from_secs(60 * 60);

pub fn queued_task_hard_timeout_secs() -> u64 {
    QUEUED_TASK_HARD_TIMEOUT.as_secs()
}

/// FIFO lane identity: one worker per `(chat_id, persona_id)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct QueueLaneKey {
    pub chat_id: i64,
    pub persona_id: i64,
}

impl QueueLaneKey {
    pub fn new(chat_id: i64, persona_id: i64) -> Self {
        Self {
            chat_id,
            persona_id,
        }
    }
}

/// Where the queued work originated (for web diagnostics).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueueSource {
    Web,
    Telegram,
    Discord,
    Whatsapp,
    Wecom,
    Scheduler,
}

impl QueueSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            QueueSource::Web => "web",
            QueueSource::Telegram => "telegram",
            QueueSource::Discord => "discord",
            QueueSource::Whatsapp => "whatsapp",
            QueueSource::Wecom => "wecom",
            QueueSource::Scheduler => "scheduler",
        }
    }
}

struct QueuedTask {
    fut: BoxTaskFuture,
    enqueued_at: Instant,
    run_id: String,
    project_id: Option<i64>,
    workflow_id: Option<i64>,
    on_hard_abort: Option<QueueHardAbortHook>,
}

struct QueueItemEntry {
    run_id: String,
    persona_id: i64,
    source: QueueSource,
    label: String,
    project_id: Option<i64>,
    workflow_id: Option<i64>,
    enqueued_at: Instant,
}

struct PersonaLane {
    tx: mpsc::UnboundedSender<QueuedTask>,
    /// Number of enqueued + currently running tasks for this persona lane.
    pending: usize,
    started_at: Instant,
    last_error: Option<String>,
    current_project_id: Option<i64>,
    current_workflow_id: Option<i64>,
    oldest_enqueued_at: Option<Instant>,
    /// FIFO order: front is running or next to run.
    items: VecDeque<QueueItemEntry>,
    /// Which `run_id` is currently executing (must match `items.front()` while running).
    current_run_id: Option<String>,
    /// When `current_run_id` began executing. Cleared with the run id.
    current_run_started_at: Option<Instant>,
}

#[derive(Clone, Debug, Default)]
pub struct QueueTaskMeta {
    pub project_id: Option<i64>,
    pub workflow_id: Option<i64>,
}

/// Metadata for one enqueue; `run_id` must be unique per enqueue (e.g. UUID).
#[derive(Clone)]
pub struct QueueEnqueueMeta {
    pub run_id: String,
    pub persona_id: i64,
    pub source: QueueSource,
    pub label: String,
    pub project_id: Option<i64>,
    pub workflow_id: Option<i64>,
    /// Invoked after the lane aborts a running task (timeout / panic).
    pub on_hard_abort: Option<QueueHardAbortHook>,
}

impl std::fmt::Debug for QueueEnqueueMeta {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueueEnqueueMeta")
            .field("run_id", &self.run_id)
            .field("persona_id", &self.persona_id)
            .field("source", &self.source)
            .field("label", &self.label)
            .field("project_id", &self.project_id)
            .field("workflow_id", &self.workflow_id)
            .field("on_hard_abort", &self.on_hard_abort.as_ref().map(|_| "set"))
            .finish()
    }
}

impl QueueEnqueueMeta {
    /// For callers that only need project/workflow (legacy `QueueTaskMeta` shape).
    pub fn from_task_meta(
        run_id: String,
        persona_id: i64,
        source: QueueSource,
        label: String,
        m: QueueTaskMeta,
    ) -> Self {
        Self {
            run_id,
            persona_id,
            source,
            label,
            project_id: m.project_id,
            workflow_id: m.workflow_id,
            on_hard_abort: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct QueueItemDiagnostic {
    pub run_id: String,
    pub persona_id: i64,
    pub source: String,
    pub label: String,
    pub state: String,
    pub project_id: Option<i64>,
    pub workflow_id: Option<i64>,
    pub position: usize,
    /// Running: time since execution started. Queued: time waiting since enqueue.
    pub elapsed_ms: u128,
}

#[derive(Clone, Debug)]
pub struct LaneDiagnostic {
    pub chat_id: i64,
    pub persona_id: i64,
    pub pending: usize,
    pub active_for_ms: u128,
    pub oldest_wait_ms: u128,
    pub last_error: Option<String>,
    pub project_id: Option<i64>,
    pub workflow_id: Option<i64>,
    pub items: Vec<QueueItemDiagnostic>,
}

#[derive(Clone, Default)]
pub struct ChatRunQueue {
    lanes: Arc<Mutex<HashMap<QueueLaneKey, PersonaLane>>>,
    /// `run_id` -> (lane key, cancel flag) for `request_cancel`.
    runs: Arc<Mutex<RunRegistry>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueRemoveOutcome {
    Removed,
    Running,
    NotFound,
}

impl ChatRunQueue {
    /// Enqueue a task for a persona-scoped FIFO lane (`persona_id` must be set in meta).
    /// Returns 1-based queue position and the cancel handle for cooperative cancellation.
    pub async fn enqueue<F, Fut>(&self, chat_id: i64, make_fut: F) -> (usize, Arc<AtomicBool>)
    where
        F: FnOnce(Arc<AtomicBool>) -> Fut,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let run_id = uuid::Uuid::new_v4().to_string();
        let meta = QueueEnqueueMeta::from_task_meta(
            run_id,
            0,
            QueueSource::Web,
            String::new(),
            QueueTaskMeta::default(),
        );
        self.enqueue_with_meta(chat_id, meta, make_fut).await
    }

    pub async fn enqueue_with_meta<F, Fut>(
        &self,
        chat_id: i64,
        meta: QueueEnqueueMeta,
        make_fut: F,
    ) -> (usize, Arc<AtomicBool>)
    where
        F: FnOnce(Arc<AtomicBool>) -> Fut,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let lane_key = QueueLaneKey::new(chat_id, meta.persona_id);
        if lane_key.persona_id <= 0 {
            warn!(
                chat_id,
                persona_id = meta.persona_id,
                "chat queue enqueue rejected: persona_id must be positive"
            );
            let cancel = Arc::new(AtomicBool::new(false));
            return (0, cancel);
        }

        let cancel = Arc::new(AtomicBool::new(false));
        let run_id = meta.run_id.clone();
        {
            let mut guard = self.runs.lock().await;
            guard.insert(run_id.clone(), (lane_key, cancel.clone()));
        }

        let mut entry = QueueItemEntry {
            run_id: run_id.clone(),
            persona_id: meta.persona_id,
            source: meta.source.clone(),
            label: meta.label.clone(),
            project_id: meta.project_id,
            workflow_id: meta.workflow_id,
            enqueued_at: Instant::now(),
        };

        let fut = make_fut(cancel.clone());

        let mut lanes = self.lanes.lock().await;
        let lane =
            if let Some(existing) = lanes.get_mut(&lane_key) {
                existing
            } else {
                let queue = self.clone();
                let (tx, mut rx) = mpsc::unbounded_channel::<QueuedTask>();
                let lane_key_worker = lane_key;
                tokio::spawn(async move {
                    while let Some(task) = rx.recv().await {
                        let started_wait = task.enqueued_at.elapsed();
                        if started_wait.as_secs() >= 300 {
                            warn!(
                                chat_id = lane_key_worker.chat_id,
                                persona_id = lane_key_worker.persona_id,
                                wait_ms = started_wait.as_millis(),
                                project_id = task.project_id,
                                workflow_id = task.workflow_id,
                                "queued task waited a long time before starting"
                            );
                        }
                        let run_id = task.run_id.clone();
                        let skip = {
                            let guard = queue.runs.lock().await;
                            guard
                                .get(&run_id)
                                .map(|(_, c)| c.load(Ordering::SeqCst))
                                .unwrap_or(false)
                        };
                        if skip {
                            queue.finish_one(lane_key_worker, &run_id).await;
                            continue;
                        }
                        {
                            let mut lanes = queue.lanes.lock().await;
                            if let Some(lane) = lanes.get_mut(&lane_key_worker) {
                                lane.current_run_id = Some(run_id.clone());
                                lane.current_run_started_at = Some(Instant::now());
                            }
                        }

                        // Isolate each queue task so a panic cannot kill the lane worker.
                        let mut task_handle = tokio::spawn(task.fut);
                        let abort_hook = task.on_hard_abort.clone();
                        let abort_reason =
                            match tokio::time::timeout(QUEUED_TASK_HARD_TIMEOUT, &mut task_handle)
                                .await
                            {
                                Ok(Ok(())) => None,
                                Ok(Err(e)) => {
                                    let msg = if e.is_panic() {
                                        "queued task panicked; lane recovered".to_string()
                                    } else {
                                        "queued task join failed; lane recovered".to_string()
                                    };
                                    warn!(
                                        chat_id = lane_key_worker.chat_id,
                                        persona_id = lane_key_worker.persona_id,
                                        run_id = %run_id,
                                        error = %e,
                                        "{msg}"
                                    );
                                    queue.note_lane_error(lane_key_worker, msg.clone()).await;
                                    Some(msg)
                                }
                                Err(_) => {
                                    // Prefer cooperative cancel so the agent can notice, then abort.
                                    {
                                        let guard = queue.runs.lock().await;
                                        if let Some((_, c)) = guard.get(&run_id) {
                                            c.store(true, Ordering::SeqCst);
                                        }
                                    }
                                    task_handle.abort();
                                    let _ = task_handle.await;
                                    let msg = format!(
                                        "queued task exceeded hard timeout ({}s) and was cancelled",
                                        QUEUED_TASK_HARD_TIMEOUT.as_secs()
                                    );
                                    warn!(
                                        chat_id = lane_key_worker.chat_id,
                                        persona_id = lane_key_worker.persona_id,
                                        run_id = %run_id,
                                        timeout_secs = QUEUED_TASK_HARD_TIMEOUT.as_secs(),
                                        "{msg}"
                                    );
                                    queue.note_lane_error(lane_key_worker, msg.clone()).await;
                                    Some(crate::queue_abort::hard_timeout_user_message(
                                        QUEUED_TASK_HARD_TIMEOUT.as_secs(),
                                    ))
                                }
                            };

                        if let (Some(reason), Some(hook)) = (abort_reason, abort_hook) {
                            hook(reason).await;
                        }

                        queue.finish_one(lane_key_worker, &run_id).await;
                    }
                });
                lanes.insert(
                    lane_key,
                    PersonaLane {
                        tx,
                        pending: 0,
                        started_at: Instant::now(),
                        last_error: None,
                        current_project_id: None,
                        current_workflow_id: None,
                        oldest_enqueued_at: None,
                        items: VecDeque::new(),
                        current_run_id: None,
                        current_run_started_at: None,
                    },
                );
                lanes
                    .get_mut(&lane_key)
                    .expect("lane inserted for persona queue")
            };

        lane.pending = lane.pending.saturating_add(1);
        lane.current_project_id = meta.project_id.or(lane.current_project_id);
        lane.current_workflow_id = meta.workflow_id.or(lane.current_workflow_id);
        let now = Instant::now();
        entry.enqueued_at = now;
        if lane.oldest_enqueued_at.map_or(true, |t| now < t) {
            lane.oldest_enqueued_at = Some(now);
        }
        lane.items.push_back(entry);
        let position = lane.pending;

        if lane
            .tx
            .send(QueuedTask {
                fut: Box::pin(fut),
                enqueued_at: now,
                run_id: run_id.clone(),
                project_id: meta.project_id,
                workflow_id: meta.workflow_id,
                on_hard_abort: meta.on_hard_abort.clone(),
            })
            .is_err()
        {
            lane.pending = lane.pending.saturating_sub(1);
            lane.items.pop_back();
            lane.last_error = Some("lane worker unavailable".to_string());
            warn!(
                chat_id = lane_key.chat_id,
                persona_id = lane_key.persona_id,
                "persona queue worker unavailable; task dropped"
            );
            let mut guard = self.runs.lock().await;
            guard.remove(&run_id);
            return (0, cancel);
        }
        (position, cancel)
    }

    async fn finish_one(&self, lane_key: QueueLaneKey, run_id: &str) {
        {
            let mut guard = self.runs.lock().await;
            guard.remove(run_id);
        }
        let mut lanes = self.lanes.lock().await;
        let remove_lane = if let Some(lane) = lanes.get_mut(&lane_key) {
            let was_running = lane.current_run_id.as_deref() == Some(run_id);
            if was_running {
                lane.current_run_id = None;
                lane.current_run_started_at = None;
            }
            let mut removed_from_items = false;
            if let Some(front) = lane.items.front() {
                if front.run_id == run_id {
                    lane.items.pop_front();
                    removed_from_items = true;
                }
            }
            if was_running || removed_from_items {
                lane.pending = lane.pending.saturating_sub(1);
            }
            if lane.pending == 0 {
                lane.oldest_enqueued_at = None;
            }
            lane.pending == 0
        } else {
            false
        };
        if remove_lane {
            lanes.remove(&lane_key);
        }
    }

    async fn note_lane_error(&self, lane_key: QueueLaneKey, message: String) {
        let mut lanes = self.lanes.lock().await;
        if let Some(lane) = lanes.get_mut(&lane_key) {
            lane.last_error = Some(message);
        }
    }

    /// Request cooperative cancellation for `run_id`. Returns `true` if the run was known and `chat_id` matches.
    pub async fn request_cancel(&self, run_id: &str, chat_id: i64) -> bool {
        let lane_and_cancel = {
            let guard = self.runs.lock().await;
            guard.get(run_id).and_then(|(key, c)| {
                if key.chat_id == chat_id {
                    Some((*key, c.clone()))
                } else {
                    None
                }
            })
        };
        if let Some((key, c)) = lane_and_cancel {
            c.store(true, Ordering::SeqCst);
            info!(
                chat_id = key.chat_id,
                persona_id = key.persona_id,
                run_id,
                "queue cancel requested"
            );
            return true;
        }
        warn!(chat_id, run_id, "queue cancel requested for unknown run");
        false
    }

    /// Remove a queued (not currently running) item.
    pub async fn request_remove_queued(&self, run_id: &str, chat_id: i64) -> QueueRemoveOutcome {
        let (lane_key, cancel) = {
            let guard = self.runs.lock().await;
            let Some((key, c)) = guard.get(run_id) else {
                warn!(chat_id, run_id, "queue remove requested for unknown run");
                return QueueRemoveOutcome::NotFound;
            };
            if key.chat_id != chat_id {
                warn!(chat_id, run_id, "queue remove requested for unknown run");
                return QueueRemoveOutcome::NotFound;
            }
            (*key, c.clone())
        };

        let mut lanes = self.lanes.lock().await;
        let Some(lane) = lanes.get_mut(&lane_key) else {
            warn!(
                chat_id = lane_key.chat_id,
                persona_id = lane_key.persona_id,
                run_id,
                "queue remove requested but lane missing"
            );
            return QueueRemoveOutcome::NotFound;
        };

        if lane.current_run_id.as_deref() == Some(run_id) {
            info!(
                chat_id = lane_key.chat_id,
                persona_id = lane_key.persona_id,
                run_id,
                "queue remove rejected because run is currently running"
            );
            return QueueRemoveOutcome::Running;
        }

        let idx = lane.items.iter().position(|e| e.run_id == run_id);
        let Some(idx) = idx else {
            warn!(
                chat_id = lane_key.chat_id,
                persona_id = lane_key.persona_id,
                run_id,
                "queue remove requested but run is not queued"
            );
            return QueueRemoveOutcome::NotFound;
        };

        lane.items.remove(idx);
        lane.pending = lane.pending.saturating_sub(1);
        if lane.pending == 0 {
            lane.oldest_enqueued_at = None;
        }
        cancel.store(true, Ordering::SeqCst);
        info!(
            chat_id = lane_key.chat_id,
            persona_id = lane_key.persona_id,
            run_id,
            "queue remove accepted for queued run"
        );

        if lane.pending == 0 {
            lanes.remove(&lane_key);
        }
        QueueRemoveOutcome::Removed
    }

    pub async fn diagnostics(&self) -> Vec<LaneDiagnostic> {
        let lanes = self.lanes.lock().await;
        let now = Instant::now();
        lanes
            .iter()
            .map(|(lane_key, lane)| {
                let items: Vec<QueueItemDiagnostic> = lane
                    .items
                    .iter()
                    .enumerate()
                    .map(|(i, e)| {
                        let running = Some(e.run_id.as_str()) == lane.current_run_id.as_deref();
                        let state = if running { "running" } else { "queued" };
                        let elapsed_ms = if running {
                            lane.current_run_started_at
                                .map(|t| now.duration_since(t).as_millis())
                                .unwrap_or_else(|| now.duration_since(e.enqueued_at).as_millis())
                        } else {
                            now.duration_since(e.enqueued_at).as_millis()
                        };
                        QueueItemDiagnostic {
                            run_id: e.run_id.clone(),
                            persona_id: e.persona_id,
                            source: e.source.as_str().to_string(),
                            label: e.label.clone(),
                            state: state.to_string(),
                            project_id: e.project_id,
                            workflow_id: e.workflow_id,
                            position: i + 1,
                            elapsed_ms,
                        }
                    })
                    .collect();
                LaneDiagnostic {
                    chat_id: lane_key.chat_id,
                    persona_id: lane_key.persona_id,
                    pending: lane.pending,
                    active_for_ms: now.duration_since(lane.started_at).as_millis(),
                    oldest_wait_ms: lane
                        .oldest_enqueued_at
                        .map(|t| now.duration_since(t).as_millis())
                        .unwrap_or(0),
                    last_error: lane.last_error.clone(),
                    project_id: lane.current_project_id,
                    workflow_id: lane.current_workflow_id,
                    items,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn test_meta(chat_id: i64, persona_id: i64, run_id: &str) -> QueueEnqueueMeta {
        QueueEnqueueMeta {
            run_id: run_id.to_string(),
            persona_id,
            source: QueueSource::Web,
            label: format!("chat {chat_id} persona {persona_id}"),
            project_id: None,
            workflow_id: None,
            on_hard_abort: None,
        }
    }

    #[tokio::test]
    async fn different_personas_same_chat_run_in_parallel() {
        let queue = ChatRunQueue::default();
        let chat_id = 42_i64;
        let started_a = Arc::new(AtomicUsize::new(0));
        let started_b = Arc::new(AtomicUsize::new(0));
        let done_a = Arc::new(tokio::sync::Notify::new());
        let done_b = Arc::new(tokio::sync::Notify::new());

        let sa = started_a.clone();
        let da = done_a.clone();
        let (pos_a, _) = queue
            .enqueue_with_meta(
                chat_id,
                test_meta(chat_id, 1, "run-a"),
                move |_cancel| async move {
                    sa.fetch_add(1, Ordering::SeqCst);
                    da.notified().await;
                },
            )
            .await;
        assert_eq!(pos_a, 1);

        let sb = started_b.clone();
        let db = done_b.clone();
        let (pos_b, _) = queue
            .enqueue_with_meta(
                chat_id,
                test_meta(chat_id, 2, "run-b"),
                move |_cancel| async move {
                    sb.fetch_add(1, Ordering::SeqCst);
                    db.notified().await;
                },
            )
            .await;
        assert_eq!(pos_b, 1);

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(started_a.load(Ordering::SeqCst), 1);
        assert_eq!(started_b.load(Ordering::SeqCst), 1);

        done_a.notify_waiters();
        done_b.notify_waiters();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let diag = queue.diagnostics().await;
        assert!(diag.is_empty());
    }

    #[tokio::test]
    async fn same_persona_stays_fifo() {
        let queue = ChatRunQueue::default();
        let chat_id = 99_i64;
        let persona_id = 7_i64;
        let order = Arc::new(Mutex::new(Vec::<&'static str>::new()));

        let o1 = order.clone();
        let (pos1, _) = queue
            .enqueue_with_meta(
                chat_id,
                test_meta(chat_id, persona_id, "first"),
                move |_cancel| async move {
                    o1.lock().await.push("first");
                    tokio::time::sleep(Duration::from_millis(80)).await;
                },
            )
            .await;
        assert_eq!(pos1, 1);

        let o2 = order.clone();
        let (pos2, _) = queue
            .enqueue_with_meta(
                chat_id,
                test_meta(chat_id, persona_id, "second"),
                move |_cancel| async move {
                    o2.lock().await.push("second");
                },
            )
            .await;
        assert_eq!(pos2, 2);

        tokio::time::sleep(Duration::from_millis(200)).await;
        let seen = order.lock().await.clone();
        assert_eq!(seen, vec!["first", "second"]);
    }

    #[tokio::test]
    async fn cancel_and_remove_queued() {
        let queue = ChatRunQueue::default();
        let chat_id = 1_i64;
        let persona_id = 2_i64;
        let block = Arc::new(tokio::sync::Notify::new());
        let running_started = Arc::new(AtomicBool::new(false));

        let b = block.clone();
        let rs = running_started.clone();
        let (pos_run, _) = queue
            .enqueue_with_meta(
                chat_id,
                test_meta(chat_id, persona_id, "running"),
                move |_cancel| async move {
                    rs.store(true, Ordering::SeqCst);
                    b.notified().await;
                },
            )
            .await;
        assert_eq!(pos_run, 1);

        let deadline = Instant::now() + Duration::from_secs(2);
        while !running_started.load(Ordering::SeqCst) {
            if Instant::now() >= deadline {
                panic!("running task did not start in time");
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let (pos_q, _) = queue
            .enqueue_with_meta(
                chat_id,
                test_meta(chat_id, persona_id, "queued"),
                |_cancel| async move {},
            )
            .await;
        assert_eq!(pos_q, 2);

        assert_eq!(
            queue.request_remove_queued("running", chat_id).await,
            QueueRemoveOutcome::Running
        );
        assert_eq!(
            queue.request_remove_queued("queued", chat_id).await,
            QueueRemoveOutcome::Removed
        );

        block.notify_waiters();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let lanes = queue.diagnostics().await;
        assert!(lanes.is_empty());
    }

    #[tokio::test]
    async fn enqueue_rejects_invalid_persona_id() {
        let queue = ChatRunQueue::default();
        let (pos, _) = queue
            .enqueue_with_meta(1, test_meta(1, 0, "bad"), |_cancel| async move {})
            .await;
        assert_eq!(pos, 0);
    }
}
