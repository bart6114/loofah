use crate::{
    Blob, EndpointRoot, Error, OwnerResponse, Result,
    platform::Stream,
    wire::{self, Control},
};
use hypr_loofah_protocol::{
    LocalCliOperation, LocalCliOperationKind, LocalCliProgress, LocalCliResponse,
};
use hypr_storage::vault::{LeaseAttempt, VaultLease};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

pub enum OwnerResolution {
    Acquired(VaultLease),
    Connected(OwnerClient),
}

pub struct OwnerClient {
    io: Stream,
    epoch: String,
    operations: Vec<LocalCliOperationKind>,
    temporary_root: PathBuf,
}
impl OwnerClient {
    pub async fn resolve(
        vault: &Path,
        endpoints: &EndpointRoot,
        temporary_root: &Path,
    ) -> Result<OwnerResolution> {
        let vault = vault.canonicalize()?;
        crate::validate_temporary_root(temporary_root)?;
        match VaultLease::try_acquire_existing(&vault)? {
            LeaseAttempt::Acquired(lease) => Ok(OwnerResolution::Acquired(lease)),
            LeaseAttempt::Busy => {
                let endpoint = endpoints.endpoint(&vault)?;
                let result = tokio::time::timeout(Duration::from_secs(2), async {
                    let mut io = loop {
                        match endpoint.connect().await {
                            Ok(io) => break io,
                            Err(_) => tokio::time::sleep(Duration::from_millis(25)).await,
                        }
                    };
                    wire::send_control(
                        &mut io,
                        &Control::Hello {
                            major: wire::MAJOR,
                            vault_key: endpoint.key.clone(),
                        },
                    )
                    .await?;
                    match wire::read_control(&mut io).await? {
                        Control::Welcome {
                            major,
                            vault_key,
                            epoch,
                            operations,
                        } if major == wire::MAJOR
                            && vault_key == endpoint.key
                            && wire::uuid(&epoch) =>
                        {
                            Ok(Self {
                                io,
                                epoch,
                                operations,
                                temporary_root: temporary_root.to_owned(),
                            })
                        }
                        _ => Err(Error::protocol(
                            "Local owner protocol or vault identity does not match",
                        )),
                    }
                })
                .await;
                match result {
                    Ok(Ok(client))=>Ok(OwnerResolution::Connected(client)),
                    _ => match VaultLease::try_acquire_existing(&vault)? {
                        LeaseAttempt::Acquired(lease) => Ok(OwnerResolution::Acquired(lease)),
                        LeaseAttempt::Busy => Err(Error::Unavailable(
                            "The vault has an owner, but its local endpoint is unavailable or incompatible. Wait for startup or close the owning process; no independent vault was opened.".into(),
                        )),
                    },
                }
            }
        }
    }

    pub fn supports(&self, kind: &LocalCliOperationKind) -> bool {
        self.operations.contains(kind)
    }
    pub fn epoch(&self) -> &str {
        &self.epoch
    }

    pub async fn execute(
        self,
        request_id: String,
        operation: LocalCliOperation,
        blob: Option<Blob>,
    ) -> Result<OwnerResponse> {
        self.execute_with_progress(request_id, operation, blob, |_| {})
            .await
    }

    pub async fn execute_with_progress(
        mut self,
        request_id: String,
        operation: LocalCliOperation,
        blob: Option<Blob>,
        mut progress: impl FnMut(LocalCliProgress) + Send,
    ) -> Result<OwnerResponse> {
        if !wire::uuid(&request_id) {
            return Err(Error::protocol(
                "A canonical non-nil request UUID is required",
            ));
        }
        if !self.supports(&operation.kind()) {
            return Err(Error::Unavailable(
                "This vault owner does not support the requested CLI operation yet".into(),
            ));
        }
        if operation.has_blob() != blob.is_some() {
            return Err(Error::protocol(
                "Local operation payload does not match its declaration",
            ));
        }
        let kind = operation.kind();
        let mutation = operation.is_mutation();
        let payload = Blob::encode(operation, &self.temporary_root, None).await?;
        let result = self
            .exchange(&request_id, kind, payload, blob, &mut progress)
            .await;
        if mutation && result.is_err() {
            return Err(Error::Unknown { request_id });
        }
        result
    }

    async fn exchange(
        &mut self,
        request_id: &str,
        kind: LocalCliOperationKind,
        payload: Blob,
        blob: Option<Blob>,
        progress: &mut (impl FnMut(LocalCliProgress) + Send),
    ) -> Result<OwnerResponse> {
        wire::send_control(
            &mut self.io,
            &Control::Request {
                request_id: request_id.into(),
                epoch: self.epoch.clone(),
                payload: payload.descriptor(),
                blob: blob.as_ref().map(Blob::descriptor),
            },
        )
        .await?;
        wire::send_blob(&mut self.io, &payload).await?;
        if let Some(blob) = blob {
            wire::send_blob(&mut self.io, &blob).await?;
        }
        let mut accepted = false;
        loop {
            match wire::read_control(&mut self.io).await? {
                Control::Accepted {
                    request_id: id,
                    epoch,
                } if id == request_id && epoch == self.epoch && !accepted => accepted = true,
                Control::Keepalive {
                    request_id: id,
                    epoch,
                } if id == request_id && epoch == self.epoch && accepted => {}
                Control::Progress {
                    request_id: id,
                    epoch,
                    progress: event,
                } if id == request_id && epoch == self.epoch && accepted => progress(event),
                Control::Response {
                    request_id: id,
                    epoch,
                    payload,
                    blob,
                } if id == request_id && epoch == self.epoch && accepted => {
                    let payload =
                        wire::receive_blob(&mut self.io, payload, &self.temporary_root, None)
                            .await?;
                    let response: LocalCliResponse = payload.decode().await?;
                    if response
                        .result
                        .as_ref()
                        .is_ok_and(|reply| !kind.accepts_reply(reply, blob.is_some()))
                    {
                        return Err(Error::protocol(
                            "Local owner replied with an unexpected operation or blob",
                        ));
                    }
                    if response.result.is_err() && blob.is_some() {
                        return Err(Error::protocol("Error reply has an unexpected blob"));
                    }
                    let blob = match blob {
                        Some(descriptor) => Some(
                            wire::receive_blob(
                                &mut self.io,
                                descriptor,
                                &self.temporary_root,
                                None,
                            )
                            .await?,
                        ),
                        None => None,
                    };
                    return Ok(OwnerResponse { response, blob });
                }
                Control::Rejected { message } if !accepted => {
                    return Err(Error::Unavailable(message));
                }
                _ => return Err(Error::protocol("Unexpected local owner response")),
            }
        }
    }
}
