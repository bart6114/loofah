use hypr_supervisor::{RestartBudget, RetryStrategy, spawn_with_retry};
use ractor::concurrency::Duration;
use ractor::{Actor, ActorCell, ActorRef};

use crate::actors::session::types::SessionContext;
use crate::actors::{
    ChannelMode, ListenerActor, ListenerArgs, RecArgs, RecMsg, RecorderActor, SourceActor,
    SourceArgs, SourceMsg,
};

use super::SessionState;

pub(super) type ChildJoins = std::sync::Arc<
    std::sync::Mutex<std::collections::HashMap<ractor::ActorId, tokio::task::JoinHandle<()>>>,
>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ChildKind {
    Source,
    Listener,
    Recorder,
}

pub(super) const RESTART_BUDGET: RestartBudget = RestartBudget {
    max_restarts: 3,
    max_window: Duration::from_secs(15),
    reset_after: Some(Duration::from_secs(30)),
};

const RETRY_STRATEGY: RetryStrategy = RetryStrategy {
    max_attempts: 3,
    base_delay: Duration::from_millis(100),
};

const CHILD_STOP_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) fn identify_child(state: &SessionState, cell: &ActorCell) -> Option<ChildKind> {
    if state
        .source_cell
        .as_ref()
        .is_some_and(|c| c.get_id() == cell.get_id())
    {
        return Some(ChildKind::Source);
    }
    if state
        .listener_cell
        .as_ref()
        .is_some_and(|c| c.get_id() == cell.get_id())
    {
        return Some(ChildKind::Listener);
    }
    if state
        .recorder_cell
        .as_ref()
        .is_some_and(|c| c.get_id() == cell.get_id())
    {
        return Some(ChildKind::Recorder);
    }
    None
}

pub(super) async fn spawn_source(
    supervisor_cell: ActorCell,
    ctx: &SessionContext,
    recorder_cell: Option<ActorCell>,
    listener_routing: crate::actors::source::ListenerRouting,
    joins: ChildJoins,
) -> Result<ActorRef<SourceMsg>, ractor::SpawnErr> {
    let recorder = recorder_cell.map(Into::into);
    let (source_ref, join) = Actor::spawn_linked(
        Some(match &ctx.recording {
            crate::actors::RecordingDestination::Device { key, .. } => {
                format!("source:{}", key.as_str())
            }
            _ => SourceActor::name(),
        }),
        SourceActor,
        SourceArgs {
            mic_device: None,
            onboarding: ctx.params.onboarding,
            runtime: ctx.runtime.clone(),
            audio: ctx.audio.clone(),
            session_id: ctx.params.session_id.clone(),
            listener_routing,
            recorder,
            device_ingress: ctx.device_ingress.clone(),
            device_root: match (&ctx.recording, &ctx.device_root) {
                (crate::actors::RecordingDestination::Device { key, .. }, Some(root)) => {
                    Some((key.clone(), root.clone(), supervisor_cell.get_id()))
                }
                _ => None,
            },
            capture_mode: match &ctx.recording {
                crate::actors::RecordingDestination::Device { mode, .. } => Some(*mode),
                _ => None,
            },
        },
        supervisor_cell,
    )
    .await?;
    joins.lock().unwrap().insert(source_ref.get_id(), join);
    Ok(source_ref)
}

pub(super) async fn spawn_recorder(
    supervisor_cell: ActorCell,
    ctx: &SessionContext,
    joins: ChildJoins,
) -> Result<ActorCell, ractor::SpawnErr> {
    let (recorder_ref, join): (ActorRef<RecMsg>, _) = Actor::spawn_linked(
        Some(match &ctx.recording {
            crate::actors::RecordingDestination::Device { key, .. } => {
                format!("recorder:{}", key.as_str())
            }
            _ => RecorderActor::name(),
        }),
        RecorderActor::new(),
        RecArgs {
            destination: ctx.recording.clone(),
            owner: std::sync::Arc::new(ctx.runtime.clone()),
        },
        supervisor_cell,
    )
    .await?;
    joins.lock().unwrap().insert(recorder_ref.get_id(), join);
    Ok(recorder_ref.get_cell())
}

pub(super) async fn spawn_listener(
    supervisor_cell: ActorCell,
    ctx: &SessionContext,
    stream_offset_secs: Option<f64>,
    joins: ChildJoins,
) -> Result<ActorCell, ractor::SpawnErr> {
    let mode = ChannelMode::determine(ctx.params.onboarding);
    let (listener_ref, join): (ActorRef<crate::actors::ListenerMsg>, _) = Actor::spawn_linked(
        Some(ListenerActor::name()),
        ListenerActor,
        ListenerArgs {
            runtime: ctx.runtime.clone(),
            languages: ctx.params.languages.clone(),
            onboarding: ctx.params.onboarding,
            model: ctx.params.model.clone(),
            base_url: ctx.params.base_url.clone(),
            api_key: ctx.params.api_key.clone(),
            keywords: ctx.params.keywords.clone(),
            transcription_mode: ctx.params.transcription_mode,
            mode,
            session_started_at: ctx.started_at_instant,
            session_started_at_unix: ctx.started_at_system,
            stream_offset_secs,
            session_id: ctx.params.session_id.clone(),
            participant_human_ids: ctx.params.participant_human_ids.clone(),
            self_human_id: ctx.params.self_human_id.clone(),
        },
        supervisor_cell,
    )
    .await?;
    joins.lock().unwrap().insert(listener_ref.get_id(), join);
    Ok(listener_ref.get_cell())
}

pub(super) async fn try_restart_source(
    supervisor_cell: ActorCell,
    state: &mut SessionState,
    count_against_budget: bool,
) -> bool {
    if count_against_budget && !state.source_restarts.record_restart(&RESTART_BUDGET) {
        return false;
    }

    let sup = supervisor_cell;
    let ctx = state.ctx.clone();
    let recorder_cell = state.recorder_cell.as_ref().cloned();
    let listener_routing = state.mode.listener_routing(state.listener_cell.as_ref());
    let joins = state.child_joins.clone();

    let cell = spawn_with_retry(&RETRY_STRATEGY, || {
        let sup = sup.clone();
        let ctx = ctx.clone();
        let recorder_cell = recorder_cell.clone();
        let listener_routing = listener_routing.clone();
        let joins = joins.clone();
        async move {
            let source_ref =
                spawn_source(sup, &ctx, recorder_cell, listener_routing, joins).await?;
            Ok(source_ref.get_cell())
        }
    })
    .await;

    match cell {
        Some(c) => {
            state.source_cell = Some(c);
            true
        }
        None => false,
    }
}

pub(super) async fn try_restart_recorder(
    supervisor_cell: ActorCell,
    state: &mut SessionState,
) -> bool {
    if !state.recorder_restarts.record_restart(&RESTART_BUDGET) {
        return false;
    }

    let sup = supervisor_cell;
    let ctx = state.ctx.clone();

    let joins = state.child_joins.clone();
    let cell = spawn_with_retry(&RETRY_STRATEGY, || {
        let sup = sup.clone();
        let ctx = ctx.clone();
        let joins = joins.clone();
        async move { spawn_recorder(sup, &ctx, joins).await }
    })
    .await;

    match cell {
        Some(c) => {
            state.recorder_cell = Some(c);
            sync_source_recorder(state).await;
            true
        }
        None => false,
    }
}

pub(super) async fn attach_listener_to_source(state: &SessionState) {
    if let Some(source_cell) = &state.source_cell {
        let source_ref: ActorRef<SourceMsg> = source_cell.clone().into();
        if let Err(error) = source_ref.cast(SourceMsg::SetListenerRouting(
            state.mode.listener_routing(state.listener_cell.as_ref()),
        )) {
            tracing::warn!(?error, "failed_to_attach_listener_to_source");
        }
    }
}

pub(super) async fn stop_listener(state: &mut SessionState, reason: &str) -> f64 {
    if let Some(cell) = state.listener_cell.take() {
        let replay_duration_secs = prepare_listener_refresh(state).await;
        stop_child(&cell, &state.child_joins, reason, "listener").await;
        return replay_duration_secs;
    }

    0.0
}

pub(super) async fn prepare_listener_refresh(state: &SessionState) -> f64 {
    let Some(source_cell) = &state.source_cell else {
        return 0.0;
    };

    let source_ref: ActorRef<SourceMsg> = source_cell.clone().into();
    match ractor::call_t!(source_ref, SourceMsg::PrepareListenerRefresh, 500) {
        Ok(replay) => replay.duration_secs,
        Err(error) => {
            tracing::warn!(?error, "failed_to_prepare_listener_refresh");
            if let Some(source_cell) = &state.source_cell {
                let source_ref: ActorRef<SourceMsg> = source_cell.clone().into();
                let _ = source_ref.cast(SourceMsg::SetListenerRouting(
                    state.mode.listener_routing(state.listener_cell.as_ref()),
                ));
            }
            0.0
        }
    }
}

pub(super) async fn sync_source_recorder(state: &SessionState) {
    if let Some(source_cell) = &state.source_cell {
        let source_ref: ActorRef<SourceMsg> = source_cell.clone().into();
        let recorder = state.recorder_cell.as_ref().map(|cell| cell.clone().into());
        if let Err(error) = source_ref.cast(SourceMsg::SetRecorder(recorder)) {
            tracing::warn!(?error, "failed_to_update_source_recorder");
        }
    }
}

pub(super) async fn drain_device(
    state: &mut SessionState,
) -> Result<(), crate::actors::DeviceSinkError> {
    let source = state.source_cell.as_ref().ok_or_else(|| {
        crate::actors::DeviceSinkError("Device audio source disappeared before draining".into())
    })?;
    let actor: ActorRef<SourceMsg> = source.clone().into();
    ractor::call!(actor, SourceMsg::DrainDevice).map_err(|_| {
        crate::actors::DeviceSinkError("Device audio source did not confirm draining".into())
    })??;
    Ok(())
}

pub(super) async fn finish_interrupted_recorder(state: &SessionState) {
    if let Some(cell) = &state.recorder_cell {
        let recorder: ActorRef<RecMsg> = cell.clone().into();
        let _ = ractor::call!(
            recorder,
            RecMsg::DeviceFinish,
            crate::actors::DeviceRecordingEnd::Interrupted
        );
    }
}

pub(super) async fn drain_local(state: &SessionState) -> bool {
    let Some(cell) = &state.source_cell else {
        return false;
    };
    let source: ActorRef<SourceMsg> = cell.clone().into();
    ractor::call_t!(source, SourceMsg::DrainLocal, 30_000).unwrap_or(false)
}

pub(super) async fn shutdown_children(state: &mut SessionState, reason: &str) -> bool {
    let mut stopped = true;
    if let Some(cell) = state.source_cell.take() {
        stopped &= stop_child(&cell, &state.child_joins, reason, "source").await;
    }
    if let Some(cell) = state.listener_cell.take() {
        stopped &= stop_child(&cell, &state.child_joins, reason, "listener").await;
    }
    if let Some(cell) = state.recorder_cell.take() {
        stopped &= stop_child(&cell, &state.child_joins, reason, "recorder").await;
    }
    stopped
}

async fn stop_child(cell: &ActorCell, joins: &ChildJoins, reason: &str, child: &str) -> bool {
    if let Err(error) = cell
        .stop_and_wait(Some(reason.to_string()), Some(CHILD_STOP_TIMEOUT))
        .await
    {
        tracing::warn!(?error, %child, "child_stop_and_wait_failed");
        return false;
    }
    join_child(joins, cell.get_id()).await
}

// ActorTerminated and stop_and_wait can precede actor registry cleanup.
pub(super) async fn join_child(joins: &ChildJoins, id: ractor::ActorId) -> bool {
    let join = match joins.lock() {
        Ok(mut joins) => joins.remove(&id),
        Err(_) => return false,
    };
    match join {
        Some(mut join) => match tokio::time::timeout(CHILD_STOP_TIMEOUT, &mut join).await {
            Ok(result) => result.is_ok(),
            Err(_) => {
                if let Ok(mut joins) = joins.lock() {
                    joins.insert(id, join);
                }
                false
            }
        },
        None => false,
    }
}
