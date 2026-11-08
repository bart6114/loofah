use super::*;
use std::{future::Future, mem::size_of};

fn future_bytes<A, F: Future>(_: impl FnOnce(A) -> F) -> usize {
    size_of::<F>()
}

#[test]
fn command_future_sizes() {
    // Measure the type without constructing a potentially oversized future on
    // the test thread. Windows main threads have a smaller fixed stack.
    let sizes = [
        ("run", future_bytes(run)),
        ("run_inner", future_bytes(run_inner)),
        (
            "profile_commands",
            future_bytes(|command: cli::ProfileCommand| profiles::run(command, false)),
        ),
        (
            "reader_gateway",
            future_bytes(|reader: &'static agent_reader::AgentReader| reader.command_gateway()),
        ),
        (
            "reader_list",
            future_bytes(
                |(reader, input): (
                    &'static agent_reader::AgentReader,
                    hypr_agent_access::ListMeetingsInput,
                )| reader.list(input),
            ),
        ),
        (
            "remote_begin",
            future_bytes(|target: &'static profiles::RemoteTarget| target.begin()),
        ),
        (
            "remote_read",
            future_bytes(|target: &'static profiles::RemoteTarget| {
                target.execute(|_| hypr_loofah_client::RemoteOperation::Me)
            }),
        ),
        (
            "remote_import",
            future_bytes(|session: &'static profiles::RemoteSession| {
                session.import_media(
                    std::path::PathBuf::new(),
                    None,
                    None,
                    false,
                    Default::default(),
                    false,
                )
            }),
        ),
        (
            "remote_transcribe",
            future_bytes(|session: &'static profiles::RemoteSession| {
                session.transcribe_media("session", false)
            }),
        ),
        (
            "local_import",
            future_bytes(|path: &'static std::path::Path| {
                commands::import::run(
                    path,
                    std::path::PathBuf::new(),
                    None,
                    None,
                    false,
                    Default::default(),
                    false,
                )
            }),
        ),
        (
            "gateway_attachment",
            future_bytes(|gateway: &'static command_gateway::CommandGateway| {
                gateway.attachment_file("session", "source.wav")
            }),
        ),
        (
            "gateway_attach",
            future_bytes(
                |(gateway, source): (&'static command_gateway::CommandGateway, std::fs::File)| {
                    gateway.attach("session".into(), "source.wav".into(), source)
                },
            ),
        ),
        (
            "dispatch",
            future_bytes(
                |(args, path, cache): (
                    Args,
                    &'static std::path::Path,
                    &'static hypr_search_cache::Cache,
                )| dispatch(args, path, cache),
            ),
        ),
        (
            "meetings",
            future_bytes(
                |(reader, command): (&'static agent_reader::AgentReader, cli::MeetingCommand)| {
                    commands::meetings::run_with_reader(reader, command, false)
                },
            ),
        ),
        (
            "gateway",
            future_bytes(
                |(gateway, operation): (
                    &'static command_gateway::CommandGateway,
                    hypr_loofah_protocol::LocalCliOperation,
                )| gateway.execute(operation),
            ),
        ),
        (
            "local_owner",
            future_bytes(
                |(gateway, operation): (
                    &'static local_owner::LocalGateway,
                    hypr_loofah_protocol::LocalCliOperation,
                )| gateway.execute(operation),
            ),
        ),
    ];
    for (name, bytes) in sizes {
        eprintln!("CLI future {name}: {bytes} bytes");
    }
}
