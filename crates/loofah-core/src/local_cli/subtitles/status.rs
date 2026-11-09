use super::*;
impl LocalCli {
    pub async fn subtitle_result(
        &self,
        request: &str,
    ) -> Result<Option<TranscriptWithData>, ApplicationError> {
        canonical_uuid(request)?;
        let directory = self
            .context
            .operations
            .as_ref()
            .ok_or_else(|| unknown("Local operation storage unavailable"))?
            .root
            .join(self.media_directory("local-subtitles"))
            .join(request);
        let request = request.to_owned();
        let owner = self.context.clone();
        tokio::task::spawn_blocking(move || {
            let intent = journal::load(&directory)?
                .ok_or_else(|| error(ErrorCode::NotFound, "Subtitle request not found"))?;
            if intent.request_id != request || intent.vault_id != owner.events.vault_id() {
                return Err(unknown("Original subtitle identity changed"));
            }
            validate_session_id(&intent.session_id)
                .map_err(|_| unknown("Original subtitle session changed"))?;
            if !ledger::check_file(&directory.join("parsed.json"))? {
                return Ok(None);
            }
            Ok(Some(journal::read_parsed(&directory, &intent)?.transcript))
        })
        .await
        .map_err(unknown)?
    }

    pub async fn subtitle_status(
        &self,
        request: &str,
    ) -> Result<LocalMediaStatus, ApplicationError> {
        let request_uuid = canonical_uuid(request)?;
        let root = self
            .context
            .operations
            .as_ref()
            .ok_or_else(|| unknown("Local operation storage unavailable"))?
            .root
            .join(self.media_directory("local-subtitles"));
        let directory = root.join(request);
        let path = directory.clone();
        let owner = self.context.clone();
        let intent = tokio::task::spawn_blocking(move || {
            let _owner = owner;
            journal::load(&path)
        })
        .await
        .map_err(unknown)??
        .ok_or_else(|| error(ErrorCode::NotFound, "Subtitle request not found"))?;
        if intent.request_id != request || intent.vault_id != self.context.events.vault_id() {
            return Err(unknown("Original subtitle identity changed"));
        }
        let outcome = self
            .context
            .store
            .completed_subtitle_publication(
                &root.join("publications"),
                TranscriptPublicationIdentity {
                    vault_id: intent.vault_id.clone(),
                    namespace: self.media_namespace("local-subtitles"),
                    request_id: request_uuid,
                },
            )
            .await
            .map_err(|cause| storage_error(cause, false))?;
        let mut status = LocalMediaStatus {
            request_id: request.into(),
            session_id: intent.session_id.clone(),
            kind: LocalMediaKind::ImportSubtitles,
            state: LocalMediaState::Review,
            transcript_id: None,
            word_count: None,
            error: None,
            imported: None,
        };
        if let Some(outcome) = outcome {
            let path = directory.clone();
            let original = intent.clone();
            let owner = self.context.clone();
            let words = tokio::task::spawn_blocking(move || {
                let _owner = owner;
                journal::parsed_word_count(&path, &original)
            })
            .await
            .map_err(unknown)??;
            match outcome_reply(&intent, words, outcome) {
                Ok(LocalCliOutput {
                    reply:
                        LocalCliReply::SubtitlesImported {
                            transcript_id,
                            word_count,
                            ..
                        },
                    ..
                }) => {
                    status.state = LocalMediaState::Succeeded;
                    status.transcript_id = transcript_id;
                    status.word_count = Some(word_count);
                }
                Err(mut cause) => {
                    cause.request_id = Some(request.into());
                    status.state = if cause.mutation_outcome == Some(MutationOutcome::Unknown) {
                        LocalMediaState::Review
                    } else {
                        LocalMediaState::Failed
                    };
                    status.error = Some(cause);
                }
                _ => unreachable!("subtitle reply"),
            }
        } else {
            let mut cause = unknown(
                "Original subtitle publication is incomplete; recover the original request",
            );
            cause.request_id = Some(request.into());
            status.error = Some(cause);
        }
        Ok(status)
    }
    pub async fn subtitle_statuses(
        &self,
        session: Option<&str>,
    ) -> Result<Vec<LocalMediaStatus>, ApplicationError> {
        if let Some(session) = session {
            validate_session_id(session)
                .map_err(|_| error(ErrorCode::Validation, "Invalid subtitle session"))?;
        }
        let root = self
            .context
            .operations
            .as_ref()
            .ok_or_else(|| unknown("Local operation storage unavailable"))?
            .root
            .join(self.media_directory("local-subtitles"));
        let owner = self.context.clone();
        let ids = tokio::task::spawn_blocking(move || {
            let _owner = owner;
            if !ledger::existing_directory(&root)? {
                return Ok(Vec::new());
            }
            let mut ids = Vec::new();
            let mut bytes = 0u64;
            for (count, entry) in std::fs::read_dir(&root).map_err(unknown)?.enumerate() {
                if count > 4096 {
                    return Err(unknown("Subtitle inventory exceeds its entry budget"));
                }
                let entry = entry.map_err(unknown)?;
                let name = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| unknown("Unknown subtitle inventory path"))?;
                if name == "publications" {
                    continue;
                }
                canonical_uuid(&name)?;
                if !entry.file_type().map_err(unknown)?.is_dir() {
                    return Err(unknown("Unsafe subtitle intent directory"));
                }
                if !ledger::existing_directory(&entry.path())? {
                    return Err(unknown("Subtitle intent disappeared"));
                }
                let path = entry.path().join("intent.json");
                if !ledger::check_file(&path)? {
                    continue;
                }
                bytes = bytes.saturating_add(std::fs::metadata(&path).map_err(unknown)?.len());
                if bytes > 64 * 1024 * 1024 {
                    return Err(unknown("Subtitle inventory exceeds its byte budget"));
                }
                ids.push(name);
            }
            ids.sort();
            Ok(ids)
        })
        .await
        .map_err(unknown)??;
        let mut statuses = Vec::new();
        for id in ids {
            let status = self.subtitle_status(&id).await?;
            if session.is_none_or(|session| session == status.session_id) {
                statuses.push(status)
            }
        }
        Ok(statuses)
    }
}
