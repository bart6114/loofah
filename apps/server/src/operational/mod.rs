use crate::{
    error::{ApiError, Result},
    routes::{ApiPath, IdentityExtension},
    service::Service,
};
use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use hypr_loofah_core::operational::OperationalSnapshot;
use hypr_loofah_protocol::{ErrorCode, StartupPhase, VaultRole};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const MAX_VAULTS: usize = 4096;
const SAMPLE_INTERVAL: Duration = Duration::from_secs(30);
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Readiness {
    Opening,
    Ready,
    Failed,
    Stopping,
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StoragePressure {
    Normal,
    Low,
    Critical,
    Unavailable,
}

pub(crate) struct Observability {
    ready: AtomicBool,
    ready_sample_ms: AtomicU64,
    catalog_generation: AtomicU64,
    vaults: Mutex<BTreeMap<String, Arc<VaultMetrics>>>,
    pressure: Mutex<StoragePressure>,
}
pub(crate) struct VaultMetrics {
    observation: Mutex<Observation>,
    writes: AtomicU64,
    writes_completed: AtomicU64,
    write_failures: AtomicU64,
    write_unknown: AtomicU64,
    event_subscriptions: AtomicU64,
    event_reconnects: AtomicU64,
    event_resyncs: AtomicU64,
}
#[derive(Clone, Serialize)]
struct Observation {
    readiness: Readiness,
    sampled_at_ms: u64,
    #[serde(skip)]
    sampled_at: Option<Instant>,
    durable: Option<OperationalSnapshot>,
}
#[derive(Serialize)]
struct Snapshot {
    observation: Observation,
    sample_stale: bool,
    storage_pressure: StoragePressure,
    admitted_writes: u64,
    completed_writes: u64,
    failed_writes: u64,
    unknown_write_outcomes: u64,
    event_subscriptions: u64,
    event_reconnects: u64,
    event_resyncs: u64,
}
impl Default for Observability {
    fn default() -> Self {
        Self {
            ready: AtomicBool::new(false),
            ready_sample_ms: AtomicU64::new(0),
            catalog_generation: AtomicU64::new(u64::MAX),
            vaults: Default::default(),
            pressure: Mutex::new(StoragePressure::Unavailable),
        }
    }
}
impl Default for VaultMetrics {
    fn default() -> Self {
        Self {
            observation: Mutex::new(Observation {
                readiness: Readiness::Opening,
                sampled_at_ms: 0,
                sampled_at: None,
                durable: None,
            }),
            writes: AtomicU64::new(0),
            writes_completed: AtomicU64::new(0),
            write_failures: AtomicU64::new(0),
            write_unknown: AtomicU64::new(0),
            event_subscriptions: AtomicU64::new(0),
            event_reconnects: AtomicU64::new(0),
            event_resyncs: AtomicU64::new(0),
        }
    }
}
fn increment(value: &AtomicU64) {
    let _ = value.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
        Some(value.saturating_add(1))
    });
}
fn elapsed_ms() -> u64 {
    static STARTED: OnceLock<Instant> = OnceLock::new();
    STARTED
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}
impl Observability {
    pub(crate) fn entry(&self, id: &str) -> Option<Arc<VaultMetrics>> {
        self.vaults.lock().unwrap().get(id).cloned()
    }
    fn synchronize(&self, ids: &[String]) -> bool {
        if ids.len() > MAX_VAULTS {
            self.ready.store(false, Ordering::Release);
            return false;
        }
        let mut vaults = self.vaults.lock().unwrap();
        vaults.retain(|id, _| ids.contains(id));
        for id in ids {
            vaults.entry(id.clone()).or_default();
        }
        true
    }
    pub(crate) fn is_ready(&self, generation: u64) -> bool {
        self.ready.load(Ordering::Acquire)
            && self.catalog_generation.load(Ordering::Acquire) == generation
            && elapsed_ms().saturating_sub(self.ready_sample_ms.load(Ordering::Acquire)) <= 90_000
    }
    pub(crate) fn request_scope(
        self: &Arc<Self>,
        path: &str,
        mutation: bool,
    ) -> Option<RequestObservation> {
        let mut pieces = path
            .strip_prefix("/api/v1/vaults/")
            .or_else(|| path.strip_prefix("/vaults/"))?
            .split('/');
        let vault = pieces.next()?;
        uuid::Uuid::parse_str(vault).ok()?;
        Some(RequestObservation {
            metrics: self.clone(),
            vault: vault.into(),
            mutation,
        })
    }
}
#[derive(Clone)]
pub(crate) struct RequestObservation {
    metrics: Arc<Observability>,
    vault: String,
    mutation: bool,
}
tokio::task_local! { pub(crate) static REQUEST: Option<RequestObservation>; }
pub(crate) fn admitted_write() -> Option<Arc<VaultMetrics>> {
    REQUEST
        .try_with(|request| {
            request
                .as_ref()
                .filter(|request| request.mutation)
                .and_then(|request| request.metrics.entry(&request.vault))
        })
        .ok()
        .flatten()
        .inspect(|metrics| increment(&metrics.writes))
}
impl VaultMetrics {
    pub(crate) fn complete<T>(&self, outcome: &Result<T>) {
        increment(&self.writes_completed);
        if let Err(error) = outcome {
            if matches!(error.0.code, ErrorCode::Internal | ErrorCode::Unavailable) {
                increment(&self.write_failures);
            }
            if error.0.mutation_outcome == Some(hypr_loofah_protocol::MutationOutcome::Unknown) {
                increment(&self.write_unknown);
            }
        }
    }
    pub(crate) fn subscription(&self, reconnect: bool) {
        increment(&self.event_subscriptions);
        if reconnect {
            increment(&self.event_reconnects);
        }
    }
    pub(crate) fn resync(&self) {
        increment(&self.event_resyncs);
    }
    fn observe(&self, readiness: Readiness, durable: Option<OperationalSnapshot>) {
        *self.observation.lock().unwrap() = Observation {
            readiness,
            durable,
            sampled_at_ms: now_ms(),
            sampled_at: Some(Instant::now()),
        };
    }
    fn snapshot(&self, pressure: StoragePressure) -> Snapshot {
        let observation = self.observation.lock().unwrap().clone();
        Snapshot {
            sample_stale: observation
                .sampled_at
                .is_none_or(|sampled| sampled.elapsed() > Duration::from_secs(90)),
            observation,
            storage_pressure: pressure,
            admitted_writes: self.writes.load(Ordering::Relaxed),
            completed_writes: self.writes_completed.load(Ordering::Relaxed),
            failed_writes: self.write_failures.load(Ordering::Relaxed),
            unknown_write_outcomes: self.write_unknown.load(Ordering::Relaxed),
            event_subscriptions: self.event_subscriptions.load(Ordering::Relaxed),
            event_reconnects: self.event_reconnects.load(Ordering::Relaxed),
            event_resyncs: self.event_resyncs.load(Ordering::Relaxed),
        }
    }
}
pub(crate) async fn ready(State(service): State<Arc<Service>>) -> StatusCode {
    if !service.stop.is_cancelled()
        && service.observability.is_ready(service.catalog.generation())
        && service.catalog.ready().await
    {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}
pub(crate) async fn status(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath(vault): ApiPath<String>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &vault, false).await?;
    if access.info.role != VaultRole::Owner {
        return Err(ApiError::new(
            ErrorCode::Forbidden,
            "Only vault owners can read operational status",
        ));
    }
    let metrics = service.observability.entry(&vault).ok_or_else(|| {
        ApiError::new(ErrorCode::Unavailable, "Operational sampling is not ready")
    })?;
    let snapshot = metrics.snapshot(*service.observability.pressure.lock().unwrap());
    Ok(([("cache-control", "no-store")], Json(snapshot)).into_response())
}
pub(crate) async fn run(service: std::sync::Weak<Service>) {
    let Some(initial) = service.upgrade() else {
        return;
    };
    let mut changes = initial.catalog.subscribe();
    let stop = initial.stop.clone();
    drop(initial);
    let mut interval = tokio::time::interval(SAMPLE_INTERVAL);
    loop {
        tokio::select! { biased; _ = stop.cancelled() => break, _ = interval.tick() => {}, result = changes.changed() => if result.is_err() { break; } }
        let Some(service) = service.upgrade() else {
            break;
        };
        let generation = *changes.borrow_and_update();
        let ids = service.catalog.active_vault_ids(MAX_VAULTS + 1).await;
        if !service.observability.synchronize(&ids) {
            continue;
        }
        let mut all_ready = true;
        for id in ids {
            let Some(metrics) = service.observability.entry(&id) else {
                continue;
            };
            let opened = tokio::select! { biased; _ = stop.cancelled() => return, opened = service.vault(&id) => opened };
            match opened {
                Ok(context) => {
                    if !context.startup.is_ready() {
                        tokio::select! { biased; _ = stop.cancelled() => return, _ = tokio::time::timeout(Duration::from_secs(5), context.startup.wait_until_ready()) => {} }
                    }
                    let phase = match context.startup.snapshot().phase {
                        StartupPhase::Ready => Readiness::Ready,
                        StartupPhase::Failed { .. } => Readiness::Failed,
                        StartupPhase::Stopping | StartupPhase::Stopped => Readiness::Stopping,
                        _ => Readiness::Opening,
                    };
                    all_ready &= phase == Readiness::Ready;
                    let mut durable = context.operational_snapshot();
                    durable.media_uploads = context.sample_operational_media().await;
                    metrics.observe(phase, Some(durable));
                }
                Err(_) => {
                    all_ready = false;
                    metrics.observe(Readiness::Failed, None);
                }
            }
            tokio::task::yield_now().await;
        }
        service
            .observability
            .catalog_generation
            .store(generation, Ordering::Release);
        service
            .observability
            .ready_sample_ms
            .store(elapsed_ms(), Ordering::Release);
        service
            .observability
            .ready
            .store(all_ready && !stop.is_cancelled(), Ordering::Release);
        let owner = service.clone();
        let pressure = tokio::task::spawn_blocking(move || {
            pressure(&owner.config.storage_root, &owner.config.cache_root)
        })
        .await
        .unwrap_or(StoragePressure::Unavailable);
        *service.observability.pressure.lock().unwrap() = pressure;
    }
}
fn pressure(storage: &std::path::Path, cache: &std::path::Path) -> StoragePressure {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let mut result = StoragePressure::Normal;
    for path in [storage, cache] {
        let Some(disk) = disks
            .iter()
            .filter(|disk| path.starts_with(disk.mount_point()))
            .max_by_key(|disk| disk.mount_point().components().count())
        else {
            return StoragePressure::Unavailable;
        };
        let total = disk.total_space();
        let free = disk.available_space();
        if total == 0 {
            return StoragePressure::Unavailable;
        }
        if free < 64 * 1024 * 1024 || free < total / 100 {
            return StoragePressure::Critical;
        }
        if free < 1024 * 1024 * 1024 || free < total / 10 {
            result = StoragePressure::Low;
        }
    }
    result
}
#[cfg(test)]
mod tests;
