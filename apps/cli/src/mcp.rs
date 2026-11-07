use std::path::PathBuf;
use std::sync::Arc;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, ServiceExt, service::RequestContext, tool,
    tool_handler, tool_router,
};
use serde::Serialize;

use crate::Error;
use hypr_agent_access as access;

#[derive(Clone)]
struct LoofahMcpServer {
    vault: Arc<PathBuf>,
    cache: Arc<tokio::sync::OnceCell<hypr_search_cache::Cache>>,
}

#[derive(Debug, PartialEq, Eq)]
enum ResourceRequest {
    Meeting { meeting_id: String },
    Transcript { meeting_id: String },
}

impl LoofahMcpServer {
    fn new(vault: PathBuf, cache: Option<hypr_search_cache::Cache>) -> Self {
        Self {
            vault: Arc::new(vault),
            cache: Arc::new(tokio::sync::OnceCell::new_with(cache)),
        }
    }
    async fn cache(&self) -> std::result::Result<&hypr_search_cache::Cache, McpError> {
        self.cache
            .get_or_try_init(|| async {
                let vault = self.vault.clone();
                tokio::task::spawn_blocking(move || {
                    let cache = hypr_search_cache::Cache::for_vault(&vault)?;
                    match cache.check() {
                        Ok(()) => {}
                        Err(hypr_search_cache::Error::NotReady) => {
                            cache.initialize(&mut |_| {})?;
                        }
                        Err(error) => return Err(error),
                    }
                    Ok(cache)
                })
                .await
                .map_err(internal_error)?
                .map_err(internal_error)
            })
            .await
    }

    async fn live_call(
        &self,
        request: hypr_meeting_live::Request,
    ) -> std::result::Result<CallToolResult, McpError> {
        match crate::live::request(&self.vault, request).await {
            Ok(response) => structured(&crate::live::data(response)),
            Err(error) => {
                let mut result = CallToolResult::structured(
                    serde_json::json!({ "error": { "code": error.code, "message": error.message } }),
                );
                result.is_error = Some(true);
                Ok(result)
            }
        }
    }
}

#[tool_router]
impl LoofahMcpServer {
    #[tool(
        description = "Discover the active desktop recording and whether live transcription and nudges are available. Use its recording_id with live tools to avoid targeting another meeting.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_active_meeting(&self) -> std::result::Result<CallToolResult, McpError> {
        self.live_call(hypr_meeting_live::Request::Status).await
    }

    #[tool(
        description = "Read finalized live words and explicit corrections for a recording. Poll every five seconds using the returned cursor; drain has_more pages first. On reset replace previous context with the returned snapshot. Does not include provisional speech. Requires running macOS desktop.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_live_transcript(
        &self,
        Parameters(input): Parameters<hypr_meeting_live::TranscriptInput>,
    ) -> std::result::Result<CallToolResult, McpError> {
        self.live_call(hypr_meeting_live::Request::Transcript(input))
            .await
    }

    #[tool(
        description = "Display a plain-text coaching nudge of 1–200 characters in the active recording bar for 20 seconds, paused on hover. Replaces the previous nudge. Supply recording_id from get_active_meeting. Requires running macOS desktop with the floating bar enabled. Success means UI accepted, not that the person read it. Do not automatically retry timeouts.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn nudge(
        &self,
        Parameters(input): Parameters<hypr_meeting_live::NudgeInput>,
    ) -> std::result::Result<CallToolResult, McpError> {
        self.live_call(hypr_meeting_live::Request::Nudge(input))
            .await
    }

    #[tool(
        description = "List recent Loofah meetings with pagination metadata. Use query for relevance-ranked full-text matches, then pass next_offset as offset to continue.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn list_meetings(
        &self,
        Parameters(input): Parameters<access::ListMeetingsInput>,
    ) -> std::result::Result<CallToolResult, McpError> {
        let page = access::list_meetings(self.cache().await?, input)
            .await
            .map_err(command_error)?;
        structured(&page)
    }

    #[tool(
        description = "Get one Loofah meeting with its canonical note, summaries, and action items. Use get_meeting_transcript separately for transcript words.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_meeting(
        &self,
        Parameters(input): Parameters<access::GetMeetingInput>,
    ) -> std::result::Result<CallToolResult, McpError> {
        let meeting = access::get_meeting(&self.vault, input)
            .await
            .map_err(command_error)?;
        structured(&meeting)
    }

    #[tool(
        description = "Get the full transcript of a Loofah meeting as readable text: one '[HH:MM:SS] Speaker: ...' line per speaker turn, timed from the start of the meeting.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_meeting_transcript(
        &self,
        Parameters(input): Parameters<access::GetMeetingTranscriptInput>,
    ) -> std::result::Result<CallToolResult, McpError> {
        let page = access::get_meeting_transcript(&self.vault, input)
            .await
            .map_err(command_error)?;
        structured(&page)
    }

    #[tool(
        description = "Desktop-compatible full-text search across session titles and content. Returns one relevance-ranked result per session with its ID, score, and snippets; fetch the full transcript separately when needed.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn search_meetings(
        &self,
        Parameters(input): Parameters<access::SearchMeetingsInput>,
    ) -> std::result::Result<CallToolResult, McpError> {
        let page = access::search_meetings(self.cache().await?, input)
            .await
            .map_err(command_error)?;
        structured(&page)
    }
}

#[tool_handler]
impl ServerHandler for LoofahMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_protocol_version(ProtocolVersion::V_2024_11_05)
        .with_server_info(Implementation::new(
            "loofah",
            env!("LOOFAH_VERSION"),
        ))
        .with_instructions(
            "Local access to Loofah meeting data and live recording tools. For external coaching, discover get_active_meeting, poll get_live_transcript every five seconds with its cursor, and use nudge sparingly with the recording_id. Nudges affect the desktop UI but never change vault content. Start with list_meetings to resolve a meeting_id, then call get_meeting for notes, summaries, and action items. Call get_meeting_transcript for the full transcript as speaker-labeled '[HH:MM:SS] Speaker: ...' lines. Use search_meetings for keyword search across titles, notes, summaries, and transcript words, using desktop relevance ranking and one result per session. Never invent meeting ids, access SQLite directly, or claim a write occurred: tools never change vault content; search and list requests refresh the local search cache. Documentation: https://github.com/bart6114/loofah",
        )
    }

    async fn list_resources(
        &self,
        params: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<ListResourcesResult, McpError> {
        use rmcp::model::AnnotateAble;

        let offset = params
            .and_then(|params| params.cursor)
            .map(|cursor| {
                cursor.parse::<u32>().map_err(|_| {
                    McpError::invalid_params("resource cursor must be an integer", None)
                })
            })
            .transpose()?
            .unwrap_or(0);
        let page = access::list_meetings(
            self.cache().await?,
            access::ListMeetingsInput {
                query: None,
                limit: Some(access::DEFAULT_LIST_LIMIT),
                offset: Some(offset),
                ..Default::default()
            },
        )
        .await
        .map_err(command_error)?;
        let next_cursor = page.pagination.next_offset.map(|offset| offset.to_string());
        let resources = page
            .meetings
            .into_iter()
            .map(|meeting| {
                let name = if meeting.title.trim().is_empty() {
                    "Untitled meeting".to_string()
                } else {
                    meeting.title
                };
                RawResource::new(format!("loofah://meetings/{}", meeting.id), name)
                    .with_description("Loofah meeting context")
                    .with_mime_type("text/markdown")
                    .no_annotation()
            })
            .collect();

        Ok(ListResourcesResult {
            meta: None,
            next_cursor,
            resources,
        })
    }

    async fn list_resource_templates(
        &self,
        _params: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<ListResourceTemplatesResult, McpError> {
        use rmcp::model::AnnotateAble;

        Ok(ListResourceTemplatesResult::with_all_items(vec![
            RawResourceTemplate::new("loofah://meetings/{meeting_id}", "Loofah meeting")
                .with_description("Meeting metadata, note, summaries, and action items")
                .with_mime_type("text/markdown")
                .no_annotation(),
            RawResourceTemplate::new(
                "loofah://meetings/{meeting_id}/transcript",
                "Loofah meeting transcript",
            )
            .with_description("The full speaker-labeled meeting transcript")
            .with_mime_type("text/plain")
            .no_annotation(),
        ]))
    }

    async fn read_resource(
        &self,
        params: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<ReadResourceResult, McpError> {
        let request = parse_resource_uri(&params.uri)?;
        let contents = match request {
            ResourceRequest::Meeting { meeting_id } => {
                let meeting =
                    access::get_meeting(&self.vault, access::GetMeetingInput { meeting_id })
                        .await
                        .map_err(command_error)?;
                ResourceContents::text(meeting.to_markdown(), params.uri)
                    .with_mime_type("text/markdown")
            }
            ResourceRequest::Transcript { meeting_id } => {
                let transcript = access::get_meeting_transcript(
                    &self.vault,
                    access::GetMeetingTranscriptInput { meeting_id },
                )
                .await
                .map_err(command_error)?;
                ResourceContents::text(transcript.text, params.uri).with_mime_type("text/plain")
            }
        };

        Ok(ReadResourceResult::new(vec![contents]))
    }
}

pub async fn serve(vault: PathBuf) -> crate::Result<()> {
    let running = LoofahMcpServer::new(vault, None)
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|error| Error::operation("start MCP server", error.to_string()))?;
    running
        .waiting()
        .await
        .map_err(|error| Error::operation("run MCP server", error.to_string()))?;
    Ok(())
}

fn parse_resource_uri(uri: &str) -> std::result::Result<ResourceRequest, McpError> {
    let url =
        url::Url::parse(uri).map_err(|_| McpError::invalid_params("invalid resource URI", None))?;
    if !matches!(url.scheme(), "loofah" | "fmtr") {
        return Err(McpError::invalid_params(
            "resource URI must use the loofah or legacy fmtr scheme",
            None,
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| McpError::invalid_params("resource URI is missing a type", None))?;
    let segments = url
        .path_segments()
        .map(|segments| {
            segments
                .filter(|segment| !segment.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    match (host, segments.as_slice()) {
        ("meetings", [meeting_id]) => Ok(ResourceRequest::Meeting {
            meeting_id: (*meeting_id).to_string(),
        }),
        ("meetings", [meeting_id, "transcript"]) => Ok(ResourceRequest::Transcript {
            meeting_id: (*meeting_id).to_string(),
        }),
        _ => Err(McpError::invalid_params("unsupported resource URI", None)),
    }
}

fn structured(value: &impl Serialize) -> std::result::Result<CallToolResult, McpError> {
    serde_json::to_value(value)
        .map(CallToolResult::structured)
        .map_err(internal_error)
}

fn internal_error(error: impl std::fmt::Display) -> McpError {
    McpError::internal_error(error.to_string(), None)
}

fn command_error(error: access::Error) -> McpError {
    match error {
        access::Error::NotFound(what) => {
            McpError::invalid_params(format!("{what} not found"), None)
        }
        access::Error::InvalidInput(reason) => McpError::invalid_params(reason, None),
        other => internal_error(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn test_server(vault: &std::path::Path) -> LoofahMcpServer {
        let cache = hypr_search_cache::Cache::for_vault(vault).unwrap();
        cache.initialize(&mut |_| {}).unwrap();
        LoofahMcpServer::new(vault.to_path_buf(), Some(cache))
    }

    fn seed_vault_with_meeting() -> tempfile::TempDir {
        let vault = tempfile::tempdir().unwrap();
        let dir = vault.path().join("sessions/meeting-1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("_meta.json"),
            serde_json::json!({
                "id": "meeting-1",
                "title": "Planning",
                "started_at": "2026-07-13",
                "ended_at": null,
                "created_at": "2026-07-13T00:00:00Z",
                "tags": [],
            })
            .to_string(),
        )
        .unwrap();
        vault
    }

    #[test]
    fn parses_supported_resource_uris() {
        assert_eq!(
            parse_resource_uri("loofah://meetings/meeting-1").unwrap(),
            ResourceRequest::Meeting {
                meeting_id: "meeting-1".to_string()
            }
        );
        assert_eq!(
            parse_resource_uri("loofah://meetings/meeting-1/transcript").unwrap(),
            ResourceRequest::Transcript {
                meeting_id: "meeting-1".to_string(),
            }
        );
        assert_eq!(
            parse_resource_uri("fmtr://meetings/meeting-1").unwrap(),
            ResourceRequest::Meeting {
                meeting_id: "meeting-1".to_string()
            }
        );
        assert!(parse_resource_uri("file:///tmp/meeting").is_err());
    }

    #[tokio::test]
    async fn server_advertises_tools_and_resources() {
        let vault = tempfile::tempdir().unwrap();
        let info = test_server(vault.path()).get_info();
        assert!(info.capabilities.tools.is_some());
        assert!(info.capabilities.resources.is_some());
        let instructions = info.instructions.unwrap();
        assert!(instructions.contains("Start with list_meetings"));
        assert!(instructions.contains("https://github.com/bart6114/loofah"));
        assert!(instructions.contains("never change vault content"));
    }

    #[tokio::test]
    async fn list_tool_returns_structured_meeting_data() {
        let vault = seed_vault_with_meeting();
        let server = test_server(vault.path());

        let result = server
            .list_meetings(Parameters(access::ListMeetingsInput {
                query: Some("plan".to_string()),
                ..Default::default()
            }))
            .await
            .unwrap();

        let meetings = result.structured_content.unwrap();
        assert_eq!(meetings["meetings"][0]["id"], "meeting-1");
        assert_eq!(meetings["meetings"][0]["title"], "Planning");
        assert_eq!(meetings["pagination"]["returned"], 1);
        assert!(meetings["pagination"]["next_offset"].is_null());
    }

    #[tokio::test]
    async fn search_tool_returns_structured_hits_and_rejects_empty_input() {
        let vault = seed_vault_with_meeting();
        let server = test_server(vault.path());

        let result = server
            .search_meetings(Parameters(access::SearchMeetingsInput {
                query: "planning".to_string(),
                ..Default::default()
            }))
            .await
            .unwrap();

        let page = result.structured_content.unwrap();
        assert_eq!(page["hits"][0]["session_id"], "meeting-1");
        assert!(page["hits"][0]["score"].as_f64().unwrap() > 0.0);

        let error = server
            .search_meetings(Parameters(access::SearchMeetingsInput::default()))
            .await
            .unwrap_err();
        assert_eq!(error.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    }

    #[tokio::test]
    async fn requests_refresh_external_changes_after_server_start() {
        let vault = seed_vault_with_meeting();
        let server = test_server(vault.path());
        server
            .list_meetings(Parameters(access::ListMeetingsInput::default()))
            .await
            .unwrap();
        std::fs::write(
            vault.path().join("sessions/meeting-1/notes.md"),
            "external zebra",
        )
        .unwrap();
        let result = server
            .search_meetings(Parameters(access::SearchMeetingsInput {
                query: "zebra".into(),
                ..Default::default()
            }))
            .await
            .unwrap();
        assert_eq!(
            result.structured_content.unwrap()["hits"][0]["session_id"],
            "meeting-1"
        );
        std::fs::remove_dir_all(vault.path().join("sessions/meeting-1")).unwrap();
        let result = server
            .list_meetings(Parameters(access::ListMeetingsInput::default()))
            .await
            .unwrap();
        assert!(
            result.structured_content.unwrap()["meetings"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn client_server_handshake_lists_tools_and_resources() {
        let vault = seed_vault_with_meeting();
        let (server_transport, client_transport) = tokio::io::duplex(64 * 1024);
        let server = test_server(vault.path());
        let info = server.get_info();
        let server_handle = tokio::spawn(async move { server.serve(server_transport).await });

        let client = ().serve(client_transport).await.unwrap();
        let tools = client.list_all_tools().await.unwrap();
        let templates = client.list_all_resource_templates().await.unwrap();
        let resources = client.list_all_resources().await.unwrap();
        insta::assert_json_snapshot!(
            "mcp_contract",
            canonicalize_json(serde_json::json!({
                "protocol_version": info.protocol_version,
                "instructions": info.instructions,
                "tools": tools,
                "resource_templates": templates,
            }))
        );

        let mut tool_names = tools
            .iter()
            .map(|tool| tool.name.to_string())
            .collect::<Vec<_>>();
        tool_names.sort();
        assert_eq!(
            tool_names,
            [
                "get_active_meeting",
                "get_live_transcript",
                "get_meeting",
                "get_meeting_transcript",
                "list_meetings",
                "nudge",
                "search_meetings",
            ]
        );
        let mcp_docs = include_str!("../../../docs/src/content/docs/reference/mcp.mdx");
        let mcp_skill = include_str!("../../../skills/loofah/references/mcp.md");
        for tool_name in &tool_names {
            assert!(
                mcp_docs.contains(tool_name),
                "MCP docs are missing `{tool_name}`"
            );
            assert!(
                mcp_skill.contains(tool_name),
                "loofah skill is missing `{tool_name}`"
            );
        }
        for tool in tools {
            let properties = tool
                .input_schema
                .get("properties")
                .and_then(Value::as_object);
            for parameter in properties.into_iter().flat_map(|p| p.keys()) {
                assert!(
                    mcp_docs.contains(&format!("`{parameter}`")),
                    "MCP docs are missing `{parameter}`"
                );
            }
            let annotations = tool.annotations.expect("tool annotations");
            assert_eq!(annotations.read_only_hint, Some(tool.name != "nudge"));
            assert_eq!(annotations.destructive_hint, Some(false));
            assert_eq!(annotations.idempotent_hint, Some(tool.name != "nudge"));
            assert_eq!(annotations.open_world_hint, Some(false));
        }

        let mut template_contract = templates
            .iter()
            .map(|template| {
                (
                    template.raw.name.clone(),
                    template.raw.uri_template.clone(),
                    template.annotations.clone(),
                )
            })
            .collect::<Vec<_>>();
        template_contract.sort_by(|left, right| left.1.cmp(&right.1));
        assert_eq!(
            template_contract,
            [
                (
                    "Loofah meeting".to_string(),
                    "loofah://meetings/{meeting_id}".to_string(),
                    None,
                ),
                (
                    "Loofah meeting transcript".to_string(),
                    "loofah://meetings/{meeting_id}/transcript".to_string(),
                    None,
                ),
            ]
        );
        for (_, uri, _) in &template_contract {
            assert!(mcp_docs.contains(uri), "MCP docs are missing `{uri}`");
            assert!(mcp_skill.contains(uri), "loofah skill is missing `{uri}`");
        }
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0].raw.name, "Planning");
        assert_eq!(resources[0].raw.uri, "loofah://meetings/meeting-1");
        assert!(resources[0].annotations.is_none());

        client.cancel().await.unwrap();
        let server = server_handle.await.unwrap().unwrap();
        server.cancel().await.unwrap();
    }

    fn canonicalize_json(value: Value) -> Value {
        match value {
            Value::Object(object) => Value::Object(
                object
                    .into_iter()
                    .map(|(key, value)| (key, canonicalize_json(value)))
                    .collect::<std::collections::BTreeMap<_, _>>()
                    .into_iter()
                    .collect(),
            ),
            Value::Array(values) => {
                Value::Array(values.into_iter().map(canonicalize_json).collect())
            }
            value => value,
        }
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;

    #[tokio::test]
    async fn corrupt_cache_does_not_block_live_tools() {
        let vault = tempfile::tempdir().unwrap();
        let cache = hypr_search_cache::Cache::for_vault(vault.path()).unwrap();
        cache.initialize(&mut |_| {}).unwrap();
        std::fs::write(cache.path().join("CURRENT"), "../invalid").unwrap();
        let server = LoofahMcpServer::new(vault.path().into(), None);
        let live = server.get_active_meeting().await.unwrap();
        assert_eq!(live.is_error, Some(true));
        assert_ne!(
            live.structured_content.unwrap()["error"]["code"],
            "cache_corrupt"
        );
        assert!(server.cache.get().is_none());
        let search = server
            .search_meetings(Parameters(access::SearchMeetingsInput {
                query: "hello".into(),
                ..Default::default()
            }))
            .await
            .unwrap_err();
        assert!(search.message.contains("loof init"));
    }

    #[tokio::test]
    async fn live_tool_failure_does_not_initialize_search_cache() {
        let vault = tempfile::tempdir().unwrap();
        let server = LoofahMcpServer::new(vault.path().into(), None);
        let result = server.get_active_meeting().await.unwrap();
        assert_eq!(result.is_error, Some(true));
        assert!(server.cache.get().is_none());
        let invalid = server
            .nudge(Parameters(hypr_meeting_live::NudgeInput {
                text: " ".into(),
                recording_id: None,
            }))
            .await
            .unwrap();
        assert_eq!(invalid.is_error, Some(true));
        assert_eq!(
            invalid.structured_content.unwrap()["error"]["code"],
            "invalid_input"
        );
    }
}
