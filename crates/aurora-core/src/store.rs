use std::{
    fs::{File, OpenOptions},
    future::Future,
    io::{Read, Write},
    path::{Path, PathBuf},
    pin::Pin,
    sync::mpsc,
    thread::{self, JoinHandle},
};

use tokio::sync::oneshot;

use crate::{DecodeError, EventEnvelope, decode_jsonl, encode_envelope};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("event store is poisoned after a failed persistence operation")]
    Poisoned,
    #[error("event store contains an incomplete final record")]
    IncompleteTail,
    #[error("another writer already owns the event log")]
    WriterUnavailable,
    #[error("event log cannot be loaded: {0}")]
    Decode(#[from] DecodeError),
    #[error("persistence operation failed: {message}")]
    Persistence { message: String },
    #[error("persistence worker panicked")]
    PersistenceWorkerPanicked,
}

pub type StoreFuture<'a> = Pin<Box<dyn Future<Output = Result<(), StoreError>> + 'a>>;

pub trait EventStore {
    fn commit(&mut self, envelope: EventEnvelope) -> StoreFuture<'_>;

    fn acknowledged(&self) -> &[EventEnvelope];
}

#[derive(Debug, Default)]
pub struct InMemoryEventStore {
    acknowledged: Vec<EventEnvelope>,
}

impl InMemoryEventStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl EventStore for InMemoryEventStore {
    fn commit(&mut self, envelope: EventEnvelope) -> StoreFuture<'_> {
        Box::pin(async move {
            self.acknowledged.push(envelope);
            Ok(())
        })
    }

    fn acknowledged(&self) -> &[EventEnvelope] {
        &self.acknowledged
    }
}

#[derive(Debug)]
pub struct JsonlEventStore {
    commands: mpsc::Sender<PersistenceCommand>,
    worker: Option<JoinHandle<()>>,
    acknowledged: Vec<EventEnvelope>,
    poisoned: bool,
    #[cfg(test)]
    join_observed: Option<mpsc::Sender<()>>,
}

impl JsonlEventStore {
    pub async fn create(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        Self::start(
            path.as_ref().to_owned(),
            StartupMode::Create,
            WorkerConfiguration::default(),
        )
        .await
    }

    pub async fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        Self::start(
            path.as_ref().to_owned(),
            StartupMode::Open,
            WorkerConfiguration::default(),
        )
        .await
    }

    async fn start(
        path: PathBuf,
        mode: StartupMode,
        configuration: WorkerConfiguration,
    ) -> Result<Self, StoreError> {
        let (commands, command_rx) = mpsc::channel();
        let (startup_ack, startup_ack_rx) = oneshot::channel();
        #[cfg(test)]
        let WorkerConfiguration {
            commit_gate,
            join_observed,
        } = configuration;
        #[cfg(not(test))]
        let WorkerConfiguration {} = configuration;
        let worker = thread::Builder::new()
            .name("aurora-jsonl-persistence".to_owned())
            .spawn(move || {
                persistence_worker(
                    path,
                    mode,
                    command_rx,
                    startup_ack,
                    #[cfg(test)]
                    commit_gate,
                );
            })
            .map_err(persistence_error)?;
        let mut store = Self {
            commands,
            worker: Some(worker),
            acknowledged: Vec::new(),
            poisoned: false,
            #[cfg(test)]
            join_observed,
        };

        match startup_ack_rx.await {
            Ok(Ok(acknowledged)) => {
                store.acknowledged = acknowledged;
                Ok(store)
            }
            Ok(Err(error)) => {
                store.join_worker()?;
                Err(error)
            }
            Err(_) => match store.join_worker() {
                Err(StoreError::PersistenceWorkerPanicked) => {
                    Err(StoreError::PersistenceWorkerPanicked)
                }
                Err(error) => Err(error),
                Ok(()) => Err(worker_channel_error(
                    "initialization acknowledgement failed",
                )),
            },
        }
    }

    pub async fn close(mut self) -> Result<(), StoreError> {
        let (shutdown_ack, shutdown_ack_rx) = oneshot::channel();
        let shutdown = self
            .commands
            .send(PersistenceCommand::Shutdown { shutdown_ack })
            .map_err(|_| worker_channel_error("shutdown command could not be queued"));
        let acknowledgement = match shutdown {
            Ok(()) => shutdown_ack_rx
                .await
                .map_err(|_| worker_channel_error("shutdown acknowledgement failed")),
            Err(error) => Err(error),
        };
        let joined = self.join_worker();

        match joined {
            Err(StoreError::PersistenceWorkerPanicked) => {
                Err(StoreError::PersistenceWorkerPanicked)
            }
            Err(error) => Err(error),
            Ok(()) => acknowledgement,
        }
    }

    fn join_worker(&mut self) -> Result<(), StoreError> {
        let Some(worker) = self.worker.take() else {
            return Ok(());
        };
        let result = worker.join();
        #[cfg(test)]
        if let Some(join_observed) = self.join_observed.take() {
            let _ = join_observed.send(());
        }
        result.map_err(|_| StoreError::PersistenceWorkerPanicked)
    }

    #[cfg(test)]
    async fn create_with_test_hook(
        path: impl AsRef<Path>,
        hook: WorkerTestHook,
    ) -> Result<Self, StoreError> {
        let WorkerTestHook {
            commit_owned,
            action,
            join_observed,
        } = hook;
        Self::start(
            path.as_ref().to_owned(),
            StartupMode::Create,
            WorkerConfiguration {
                commit_gate: Some(WorkerCommitGate {
                    commit_owned,
                    action,
                }),
                join_observed: Some(join_observed),
            },
        )
        .await
    }
}

impl EventStore for JsonlEventStore {
    fn commit(&mut self, envelope: EventEnvelope) -> StoreFuture<'_> {
        Box::pin(async move {
            if self.poisoned {
                return Err(StoreError::Poisoned);
            }
            let encoded = encode_envelope(&envelope).map_err(|error| StoreError::Persistence {
                message: error.to_string(),
            })?;
            let (acknowledgement, acknowledgement_rx) = oneshot::channel();
            self.poisoned = true;
            self.commands
                .send(PersistenceCommand::Commit {
                    encoded,
                    acknowledgement,
                })
                .map_err(|_| worker_channel_error("commit command could not be queued"))?;

            match acknowledgement_rx.await {
                Ok(Ok(())) => {
                    self.acknowledged.push(envelope);
                    self.poisoned = false;
                    Ok(())
                }
                Ok(Err(error)) => Err(error),
                Err(_) => Err(worker_channel_error("commit acknowledgement failed")),
            }
        })
    }

    fn acknowledged(&self) -> &[EventEnvelope] {
        &self.acknowledged
    }
}

impl Drop for JsonlEventStore {
    fn drop(&mut self) {
        if self.worker.is_none() {
            return;
        }
        let (shutdown_ack, _shutdown_ack_rx) = oneshot::channel();
        let _ = self
            .commands
            .send(PersistenceCommand::Shutdown { shutdown_ack });
        let _ = self.join_worker();
    }
}

enum PersistenceCommand {
    Commit {
        encoded: Vec<u8>,
        acknowledgement: oneshot::Sender<Result<(), StoreError>>,
    },
    Shutdown {
        shutdown_ack: oneshot::Sender<()>,
    },
}

#[derive(Clone, Copy)]
enum StartupMode {
    Create,
    Open,
}

#[derive(Default)]
struct WorkerConfiguration {
    #[cfg(test)]
    commit_gate: Option<WorkerCommitGate>,
    #[cfg(test)]
    join_observed: Option<mpsc::Sender<()>>,
}

#[cfg(test)]
struct WorkerCommitGate {
    commit_owned: oneshot::Sender<()>,
    action: mpsc::Receiver<WorkerTestAction>,
}

fn persistence_worker(
    path: PathBuf,
    mode: StartupMode,
    commands: mpsc::Receiver<PersistenceCommand>,
    startup_ack: oneshot::Sender<Result<Vec<EventEnvelope>, StoreError>>,
    #[cfg(test)] mut commit_gate: Option<WorkerCommitGate>,
) {
    let initialized = initialize_file(&path, mode);
    let (mut file, acknowledged) = match initialized {
        Ok(initialized) => initialized,
        Err(error) => {
            let _ = startup_ack.send(Err(error));
            return;
        }
    };
    if startup_ack.send(Ok(acknowledged)).is_err() {
        return;
    }

    while let Ok(command) = commands.recv() {
        match command {
            PersistenceCommand::Commit {
                encoded,
                acknowledgement,
            } => {
                #[cfg(test)]
                run_commit_gate(&mut commit_gate);
                let result = file
                    .write_all(&encoded)
                    .and_then(|()| file.sync_all())
                    .map_err(persistence_error);
                let _ = acknowledgement.send(result);
            }
            PersistenceCommand::Shutdown { shutdown_ack } => {
                let _ = shutdown_ack.send(());
                return;
            }
        }
    }
}

fn initialize_file(
    path: &Path,
    mode: StartupMode,
) -> Result<(File, Vec<EventEnvelope>), StoreError> {
    match mode {
        StartupMode::Create => {
            let file = OpenOptions::new()
                .create_new(true)
                .read(true)
                .append(true)
                .open(path)
                .map_err(persistence_error)?;
            acquire_writer_lock(&file)?;

            // The file and its directory entry must exist durably before the
            // store becomes available to acknowledge its first record.
            file.sync_all().map_err(persistence_error)?;
            sync_parent_directory(path)?;
            Ok((file, Vec::new()))
        }
        StartupMode::Open => {
            let mut file = OpenOptions::new()
                .read(true)
                .append(true)
                .open(path)
                .map_err(persistence_error)?;
            acquire_writer_lock(&file)?;
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).map_err(persistence_error)?;
            let acknowledged = if bytes.is_empty() {
                Vec::new()
            } else {
                let decoded = decode_jsonl(&bytes)?;
                if decoded.has_incomplete_tail() {
                    return Err(StoreError::IncompleteTail);
                }
                decoded.into_envelopes()
            };
            Ok((file, acknowledged))
        }
    }
}

#[cfg(test)]
fn run_commit_gate(commit_gate: &mut Option<WorkerCommitGate>) {
    let Some(gate) = commit_gate.take() else {
        return;
    };
    let _ = gate.commit_owned.send(());
    match gate
        .action
        .recv()
        .expect("worker test controller retains the commit action sender")
    {
        WorkerTestAction::Complete => {}
        WorkerTestAction::Panic => panic!("injected persistence worker panic"),
    }
}

fn persistence_error(error: std::io::Error) -> StoreError {
    StoreError::Persistence {
        message: error.to_string(),
    }
}

fn worker_channel_error(context: &str) -> StoreError {
    StoreError::Persistence {
        message: context.to_owned(),
    }
}

fn acquire_writer_lock(file: &File) -> Result<(), StoreError> {
    match file.try_lock() {
        Ok(()) => Ok(()),
        Err(std::fs::TryLockError::WouldBlock) => Err(StoreError::WriterUnavailable),
        Err(std::fs::TryLockError::Error(error)) => Err(persistence_error(error)),
    }
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> Result<(), StoreError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(persistence_error)
}

#[cfg(not(unix))]
fn sync_parent_directory(_path: &Path) -> Result<(), StoreError> {
    Err(StoreError::Persistence {
        message: "this platform has no implemented directory-sync guarantee".to_owned(),
    })
}

#[cfg(test)]
#[derive(Debug)]
enum WorkerTestAction {
    Complete,
    Panic,
}

#[cfg(test)]
#[derive(Debug)]
struct WorkerTestHook {
    commit_owned: tokio::sync::oneshot::Sender<()>,
    action: std::sync::mpsc::Receiver<WorkerTestAction>,
    join_observed: std::sync::mpsc::Sender<()>,
}

#[cfg(test)]
#[derive(Debug)]
struct WorkerTestControl {
    commit_owned: tokio::sync::oneshot::Receiver<()>,
    action: std::sync::mpsc::Sender<WorkerTestAction>,
    join_observed: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
fn worker_test_hook() -> (WorkerTestHook, WorkerTestControl) {
    let (commit_owned, commit_owned_rx) = tokio::sync::oneshot::channel();
    let (action_tx, action) = std::sync::mpsc::channel();
    let (join_observed, join_observed_rx) = std::sync::mpsc::channel();
    (
        WorkerTestHook {
            commit_owned,
            action,
            join_observed,
        },
        WorkerTestControl {
            commit_owned: commit_owned_rx,
            action: action_tx,
            join_observed: join_observed_rx,
        },
    )
}

#[cfg(test)]
mod tests {
    use std::{sync::mpsc::TryRecvError, time::Duration};

    use tempfile::tempdir;
    use tokio_util::sync::CancellationToken;

    use super::{EventStore, JsonlEventStore, StoreError, WorkerTestAction, worker_test_hook};
    use crate::{
        AuthorizationDecision, Authorizer, DriverError, EventEnvelope, EventSeq, Inspection,
        ModelInvocation, RunDriver, RunId, RunLifecycle, RunLimits, RunStart, SCHEMA_VERSION,
        ScriptedModel, ScriptedModelStep, ToolAuthorization, ToolCatalog, inspect_jsonl,
    };

    #[derive(Debug)]
    struct AllowAll;

    impl Authorizer for AllowAll {
        fn authorize(&self, _request: &ToolAuthorization<'_>) -> AuthorizationDecision {
            AuthorizationDecision::Allow
        }
    }

    fn limits() -> RunLimits {
        RunLimits {
            max_model_steps: 1,
            max_tool_executions: 0,
            model_timeout_ms: 1_000,
            tool_timeout_ms: 1_000,
            shutdown_grace_period_ms: 100,
        }
    }

    fn start(run_id: &str) -> RunStart {
        RunStart {
            run_id: RunId::new(run_id),
            request: "request".to_owned(),
            limits: limits(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn bare_relative_jsonl_filename_syncs_the_current_directory() {
        super::sync_parent_directory(std::path::Path::new("run.jsonl"))
            .expect("a bare filename has the current directory as its durable parent");
    }

    #[tokio::test]
    async fn dropped_run_future_keeps_worker_owned_commit_until_close_joins_it() {
        let directory = tempdir().expect("temporary directory is created");
        let path = directory.path().join("dropped-run.jsonl");
        let (hook, control) = worker_test_hook();
        let mut store = JsonlEventStore::create_with_test_hook(&path, hook)
            .await
            .expect("store worker starts");
        let mut model = ScriptedModel::new(vec![ScriptedModelStep::Return(
            ModelInvocation::FinalResponse {
                text: "must not be reported".to_owned(),
            },
        )]);
        let mut catalog = ToolCatalog::empty();
        let authorizer = AllowAll;
        let mut timestamp = || "2026-01-01T00:00:00Z".to_owned();
        let mut driver = RunDriver::new(
            &mut store,
            &mut model,
            &mut catalog,
            &authorizer,
            &mut timestamp,
        );
        let mut run = Box::pin(driver.run(start("run-dropped"), CancellationToken::new()));

        tokio::select! {
            owned = control.commit_owned => owned.expect("worker takes ownership of commit"),
            result = &mut run => panic!("run reported a result before persistence completed: {result:?}"),
        }
        drop(run);
        control
            .action
            .send(WorkerTestAction::Complete)
            .expect("worker remains alive to finish its owned commit");

        let retry_error = driver
            .run(start("run-dropped"), CancellationToken::new())
            .await
            .expect_err("an unobserved acknowledgement leaves the store poisoned");
        assert!(matches!(
            retry_error,
            DriverError::Persistence(StoreError::Poisoned)
        ));
        drop(driver);
        assert!(store.acknowledged().is_empty());
        assert_eq!(model.invocation_count(), 0);

        store.close().await.expect("close joins the worker");
        control
            .join_observed
            .recv_timeout(Duration::from_secs(1))
            .expect("close observes the worker join");
        assert_eq!(
            control.join_observed.try_recv(),
            Err(TryRecvError::Disconnected)
        );

        let Inspection::Clean(reconstructed) =
            inspect_jsonl(&path).expect("the worker wrote one complete visible record")
        else {
            panic!("a complete worker write cannot be an incomplete tail");
        };
        assert_eq!(reconstructed.last_sequence, EventSeq::new(1));
        assert_eq!(reconstructed.lifecycle, RunLifecycle::Active);
    }

    #[tokio::test]
    async fn worker_panic_fails_acknowledgement_poisons_store_and_is_joined_once() {
        let directory = tempdir().expect("temporary directory is created");
        let path = directory.path().join("worker-panic.jsonl");
        let (hook, control) = worker_test_hook();
        let mut store = JsonlEventStore::create_with_test_hook(&path, hook)
            .await
            .expect("store worker starts");
        let envelope = EventEnvelope {
            schema_version: SCHEMA_VERSION,
            sequence: EventSeq::new(1),
            run_id: RunId::new("run-worker-panic"),
            observed_at: "2026-01-01T00:00:00Z".to_owned(),
            event: crate::DomainEvent::RunStarted {
                request: "request".to_owned(),
                limits: limits(),
            },
        };
        let mut commit = Box::pin(store.commit(envelope.clone()));

        tokio::select! {
            owned = control.commit_owned => owned.expect("worker takes ownership of commit"),
            result = &mut commit => panic!("commit resolved before the worker test gate: {result:?}"),
        }
        control
            .action
            .send(WorkerTestAction::Panic)
            .expect("worker remains alive at the panic gate");
        let error = commit
            .await
            .expect_err("worker panic closes the acknowledgement channel");
        assert!(matches!(error, StoreError::Persistence { .. }));
        assert!(store.acknowledged().is_empty());

        let poison_error = store
            .commit(envelope)
            .await
            .expect_err("failed acknowledgement poisons the store");
        assert!(matches!(poison_error, StoreError::Poisoned));

        let close_error = store
            .close()
            .await
            .expect_err("close reports the persistence worker panic");
        assert!(matches!(close_error, StoreError::PersistenceWorkerPanicked));
        control
            .join_observed
            .recv_timeout(Duration::from_secs(1))
            .expect("close observes the panicked worker join");
        assert_eq!(
            control.join_observed.try_recv(),
            Err(TryRecvError::Disconnected)
        );
    }

    #[tokio::test]
    async fn worker_owns_exclusive_writer_lock_until_close_joins_it() {
        let directory = tempdir().expect("temporary directory is created");
        let path = directory.path().join("exclusive-worker.jsonl");
        let first = JsonlEventStore::create(&path)
            .await
            .expect("first worker owns the writer lock");

        let error = JsonlEventStore::open(&path)
            .await
            .expect_err("second worker cannot own the same writer lock");
        assert!(matches!(error, StoreError::WriterUnavailable));

        first.close().await.expect("first worker joins");
        JsonlEventStore::open(&path)
            .await
            .expect("lock is released only after worker shutdown")
            .close()
            .await
            .expect("replacement worker joins");
    }

    #[tokio::test]
    async fn drop_fallback_joins_a_worker_owned_commit_before_returning() {
        let directory = tempdir().expect("temporary directory is created");
        let path = directory.path().join("drop-fallback.jsonl");
        let (hook, control) = worker_test_hook();
        let mut store = JsonlEventStore::create_with_test_hook(&path, hook)
            .await
            .expect("store worker starts");
        let envelope = EventEnvelope {
            schema_version: SCHEMA_VERSION,
            sequence: EventSeq::new(1),
            run_id: RunId::new("run-drop-fallback"),
            observed_at: "2026-01-01T00:00:00Z".to_owned(),
            event: crate::DomainEvent::RunStarted {
                request: "request".to_owned(),
                limits: limits(),
            },
        };
        let mut commit = Box::pin(store.commit(envelope));

        tokio::select! {
            owned = control.commit_owned => owned.expect("worker takes ownership of commit"),
            result = &mut commit => panic!("commit resolved before the worker test gate: {result:?}"),
        }
        drop(commit);
        control
            .action
            .send(WorkerTestAction::Complete)
            .expect("worker remains alive to finish its owned commit");

        drop(store);
        control
            .join_observed
            .recv_timeout(Duration::from_secs(1))
            .expect("Drop observes the worker join before returning");
        assert_eq!(
            control.join_observed.try_recv(),
            Err(TryRecvError::Disconnected)
        );

        JsonlEventStore::open(&path)
            .await
            .expect("Drop releases the worker-owned writer lock")
            .close()
            .await
            .expect("replacement worker joins");
    }
}
