use crate::{
    Blob, EndpointRoot, Error, OwnerHandler, OwnerRequest, Result,
    platform::Stream,
    wire::{self, Control},
};
use hypr_loofah_protocol::LocalCliOperation;
use hypr_storage::vault::VaultLease;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::{Semaphore, watch};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy)]
enum Completion {
    Running,
    Clean,
    Uncertain,
}

pub struct OwnerServer {
    stop: CancellationToken,
    finished: watch::Receiver<Completion>,
}
impl OwnerServer {
    pub async fn bind(
        lease: VaultLease,
        endpoints: &EndpointRoot,
        temporary_root: &Path,
        handler: Arc<dyn OwnerHandler>,
    ) -> Result<Self> {
        crate::validate_temporary_root(temporary_root)?;
        let endpoint = endpoints.endpoint(lease.canonical_vault())?;
        let mut listener = endpoint.bind(&lease).await?;
        let epoch = uuid::Uuid::new_v4().to_string();
        let stop = CancellationToken::new();
        let stopped = stop.clone();
        let temporary_root = temporary_root.to_owned();
        let (finished, receiver) = watch::channel(Completion::Running);
        tokio::spawn(async move {
            let slots = Arc::new(Semaphore::new(4));
            let mut tasks = tokio::task::JoinSet::new();
            let mut uncertain = false;
            loop {
                tokio::select! {
                    _=stopped.cancelled()=>break,
                    done=tasks.join_next(),if !tasks.is_empty()=>{
                        if lost_worker(done) { uncertain=true; stopped.cancel(); }
                    },
                    accepted=listener.accept()=>{
                        let io=match accepted {
                            Ok(io)=>io,
                            Err(_)=>{
                                tokio::select! {_=stopped.cancelled()=>break,_=tokio::time::sleep(Duration::from_millis(50))=>{}}
                                continue;
                            },
                        };
                        let Ok(slot)=slots.clone().try_acquire_owned() else {drop(io);continue;};
                        let handler=handler.clone();
                        let key=endpoint.key.clone();
                        let epoch=epoch.clone();
                        let root=temporary_root.clone();
                        let pin=crate::owner_pin(&lease);
                        let stop=stopped.clone();
                        tasks.spawn(async move {
                            let _slot=slot;
                            serve(io,key,epoch,root,handler,pin,stop).await
                        });
                    },
                }
            }
            drop(listener);
            while let Some(result) = tasks.join_next().await {
                uncertain |= lost_worker(Some(result));
            }
            if uncertain {
                let _ = finished.send(Completion::Uncertain);
                let _lease = lease;
                std::future::pending::<()>().await;
            } else {
                drop(lease);
                let _ = finished.send(Completion::Clean);
            }
        });
        Ok(Self {
            stop,
            finished: receiver,
        })
    }

    pub fn request_shutdown(&self) {
        self.stop.cancel();
    }

    pub async fn shutdown(&self) -> Result<()> {
        self.request_shutdown();
        self.wait_for_shutdown().await
    }
    pub async fn wait_for_shutdown(&self) -> Result<()> {
        let mut finished = self.finished.clone();
        loop {
            match *finished.borrow_and_update() {
                Completion::Clean => return Ok(()),
                Completion::Uncertain => return Err(Error::ApplicationWorkerLost),
                Completion::Running => {}
            }
            finished
                .changed()
                .await
                .map_err(|_| Error::ApplicationWorkerLost)?;
        }
    }
}
impl Drop for OwnerServer {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

async fn read_control(io: &mut Stream, stop: &CancellationToken) -> Result<Control> {
    tokio::select! { _=stop.cancelled()=>Err(Error::Stopped),result=wire::read_control(io)=>result }
}

async fn serve(
    mut io: Stream,
    vault_key: String,
    epoch: String,
    root: PathBuf,
    handler: Arc<dyn OwnerHandler>,
    pin: Arc<std::fs::File>,
    stop: CancellationToken,
) -> Result<()> {
    match read_control(&mut io, &stop).await? {
        Control::Hello {
            major,
            vault_key: key,
        } if major == wire::MAJOR && key == vault_key => {}
        _ => {
            send_control(
                &mut io,
                &Control::Rejected {
                    message: "Incompatible local owner or vault identity".into(),
                },
                &stop,
            )
            .await?;
            return Ok(());
        }
    }
    send_control(
        &mut io,
        &Control::Welcome {
            major: wire::MAJOR,
            vault_key,
            epoch: epoch.clone(),
            operations: handler.operations(),
        },
        &stop,
    )
    .await?;
    let (request_id, payload, blob) = match read_control(&mut io, &stop).await? {
        Control::Request {
            request_id,
            epoch: requested,
            payload,
            blob,
        } if requested == epoch && wire::uuid(&request_id) => (request_id, payload, blob),
        _ => return Err(Error::protocol("Invalid local owner request")),
    };
    let payload =
        wire::receive_blob_cancellable(&mut io, payload, &root, Some(pin.clone()), &stop).await?;
    let operation: LocalCliOperation = payload.decode().await?;
    if operation.has_blob() != blob.is_some() || !handler.operations().contains(&operation.kind()) {
        return Err(Error::protocol("Unsupported local operation or payload"));
    }
    let blob = match blob {
        Some(descriptor) => Some(
            wire::receive_blob_cancellable(&mut io, descriptor, &root, Some(pin.clone()), &stop)
                .await?,
        ),
        None => None,
    };
    if stop.is_cancelled() {
        return Err(Error::Stopped);
    }
    // Once dispatched, losing the client never cancels actual filesystem/native work.
    let (progress, mut progress_events, completion) = crate::progress::channel();
    let mut progress_open = true;
    let operation = handler.execute(OwnerRequest {
        request_id: request_id.clone(),
        operation,
        blob,
        progress,
    });
    let mut operation = tokio::spawn(async move {
        let _completion = completion;
        operation.await
    });
    let mut delivery = true;
    if send_control(
        &mut io,
        &Control::Accepted {
            request_id: request_id.clone(),
            epoch: epoch.clone(),
        },
        &stop,
    )
    .await
    .is_err()
    {
        delivery = false;
    }
    let mut heartbeat = tokio::time::interval(Duration::from_secs(2));
    heartbeat.tick().await;
    let output = loop {
        tokio::select! {
            result=&mut operation=>break result.map_err(|_|Error::ApplicationWorkerLost)?,
            event=progress_events.recv(),if delivery && progress_open=>{
                match event {
                    Some(progress)=>if send_control(&mut io,&Control::Progress{request_id:request_id.clone(),epoch:epoch.clone(),progress},&stop).await.is_err() {delivery=false;},
                    None=>progress_open=false,
                }
            },
            _=heartbeat.tick(),if delivery=>{
                if stop.is_cancelled()||send_control(&mut io,&Control::Keepalive{request_id:request_id.clone(),epoch:epoch.clone()}, &stop).await.is_err() {delivery=false;}
            },
        }
    };
    // Completion may win select with progress still queued. Close before any I/O,
    // then flush this fixed snapshot; lingering reporter clones cannot extend it.
    let pending = progress_events.finish();
    if !delivery || stop.is_cancelled() {
        return Ok(());
    }
    for progress in pending {
        send_control(
            &mut io,
            &Control::Progress {
                request_id: request_id.clone(),
                epoch: epoch.clone(),
                progress,
            },
            &stop,
        )
        .await?;
    }
    if output.response.result.is_err() && output.blob.is_some() {
        return Err(Error::protocol("Application error must not include a blob"));
    }
    let response = Blob::encode(output.response, &root, Some(pin)).await?;
    send_control(
        &mut io,
        &Control::Response {
            request_id,
            epoch,
            payload: response.descriptor(),
            blob: output.blob.as_ref().map(Blob::descriptor),
        },
        &stop,
    )
    .await?;
    wire::send_blob_cancellable(&mut io, &response, &stop).await?;
    if let Some(blob) = output.blob {
        wire::send_blob_cancellable(&mut io, &blob, &stop).await?;
    }
    Ok(())
}

fn lost_worker(result: Option<std::result::Result<Result<()>, tokio::task::JoinError>>) -> bool {
    matches!(
        result,
        Some(Err(_)) | Some(Ok(Err(Error::ApplicationWorkerLost)))
    )
}

async fn send_control(io: &mut Stream, control: &Control, stop: &CancellationToken) -> Result<()> {
    tokio::select! {_=stop.cancelled()=>Err(Error::Stopped),result=wire::send_control(io,control)=>result}
}
