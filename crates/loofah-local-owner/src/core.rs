use std::{path::PathBuf, sync::Arc};

use hypr_loofah_core::local_cli::{LocalCli, LocalCliArtifact};
use hypr_loofah_protocol::{ApplicationError, ErrorCode, LocalCliOperationKind, LocalCliResponse};

use crate::{Blob, OwnerFuture, OwnerHandler, OwnerRequest, OwnerResponse};

pub struct CoreOwner {
    dispatcher: Arc<LocalCli>,
    temporary: PathBuf,
}

impl CoreOwner {
    pub fn new(dispatcher: Arc<LocalCli>, temporary: PathBuf) -> Self {
        Self {
            dispatcher,
            temporary,
        }
    }
}

impl OwnerHandler for CoreOwner {
    fn operations(&self) -> Vec<LocalCliOperationKind> {
        self.dispatcher.operations()
    }

    fn execute(&self, request: OwnerRequest) -> OwnerFuture {
        let dispatcher = self.dispatcher.clone();
        let temporary = self.temporary.clone();
        Box::pin(async move {
            let OwnerRequest {
                request_id,
                operation,
                blob,
                progress,
            } = request;
            let result = async {
                let input = match blob {
                    Some(blob) => {
                        let size = blob.len();
                        let sha = blob.sha256().to_owned();
                        let path = blob.trusted_path().map_err(transport_error)?;
                        let file = blob
                            .with_file(|file| Ok(file.try_clone()?))
                            .await
                            .map_err(transport_error)?;
                        Some(LocalCliArtifact::new(
                            file,
                            size,
                            sha,
                            Some(path),
                            Box::new(blob),
                        ))
                    }
                    None => None,
                };
                let progress = Arc::new(move |event| progress.report(event));
                let result = dispatcher
                    .execute(request_id, operation, input, progress)
                    .await?;
                let blob = match result.artifact {
                    Some(artifact) => {
                        let (file, owner) = artifact.into_parts();
                        Some(
                            Blob::copy_from_owned(file, &temporary, owner)
                                .await
                                .map_err(transport_error)?,
                        )
                    }
                    None => None,
                };
                Ok::<_, ApplicationError>((result.reply, blob))
            }
            .await;
            match result {
                Ok((reply, blob)) => OwnerResponse {
                    response: LocalCliResponse { result: Ok(reply) },
                    blob,
                },
                Err(cause) => OwnerResponse {
                    response: LocalCliResponse { result: Err(cause) },
                    blob: None,
                },
            }
        })
    }
}

fn transport_error(cause: crate::Error) -> ApplicationError {
    ApplicationError {
        code: ErrorCode::Unavailable,
        message: cause.to_string(),
        request_id: None,
        retryable: false,
        current_revision: None,
        mutation_outcome: None,
    }
}
