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
