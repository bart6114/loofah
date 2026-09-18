use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

use hypr_agent_access::{DEFAULT_SEARCH_LIMIT, MAX_SEARCH_LIMIT, SearchKind};

#[derive(Debug, Parser)]
#[command(
    name = "loof",
    version = env!("LOOFAH_VERSION"),
    about = "Query and edit local Loofah meeting data",
    after_help = "Agent guidance lives in AGENTS.md at the vault root; run 'loof doctor' to see the vault path and restore the file if stale."
)]
pub struct Args {
    #[arg(
        long,
        global = true,
        env = "LOOFAH_BASE",
        hide_env_values = true,
        value_name = "DIR"
    )]
    pub base: Option<PathBuf>,

    #[arg(
        long,
        global = true,
        env = "LOOFAH_VAULT_PATH",
        hide_env_values = true,
        value_name = "DIR"
    )]
    pub vault_path: Option<PathBuf>,

    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Manage invitation-only staging synchronization
    Sync(crate::commands::sync::Args),
    /// Check vault access and layout; refresh the vault guide. Open desktop to migrate.
    Doctor,
    /// Browse, create, edit, and export sessions
    #[command(name = "sessions", alias = "meetings")]
    Sessions {
        #[command(subcommand)]
        command: MeetingCommand,
    },
    /// Import an audio file into a new or existing meeting and print its id
    Import {
        /// Audio file to import (wav, mp3, ogg, mp4, m4a, flac, webm, or aac)
        file: PathBuf,
        #[arg(long, help = "Title for the new meeting; defaults to the file name")]
        title: Option<String>,
        #[arg(
            long,
            value_name = "MEETING_ID",
            conflicts_with = "title",
            help = "Import into this existing meeting instead of creating a new one; fails if it already has a recording"
        )]
        into: Option<String>,
        #[arg(
            long,
            help = "Transcribe the audio with the configured on-device model after importing"
        )]
        transcribe: bool,
        #[arg(
            long,
            value_name = "RFC3339",
            value_parser = parse_rfc3339_utc,
            conflicts_with = "into",
            help = "Creation timestamp for the new meeting (RFC 3339); defaults to now"
        )]
        created_at: Option<String>,
        #[arg(
            long,
            value_name = "RFC3339",
            value_parser = parse_rfc3339_utc,
            conflicts_with = "into",
            help = "Start timestamp for the new meeting (RFC 3339)"
        )]
        started_at: Option<String>,
        #[arg(
            long,
            value_name = "RFC3339",
            value_parser = parse_rfc3339_utc,
            conflicts_with = "into",
            help = "End timestamp for the new meeting (RFC 3339)"
        )]
        ended_at: Option<String>,
        #[arg(
            long,
            value_name = "NAME",
            conflicts_with = "into",
            help = "Record who wrote the new meeting's note; set this when writing on behalf of someone other than the vault owner (e.g. claude-code)"
        )]
        author: Option<String>,
        #[arg(
            long,
            value_name = "NAME",
            conflicts_with = "into",
            requires = "author",
            help = "Record the skill used to produce the new meeting's note (e.g. meeting-summarizer); only meaningful together with --author"
        )]
        skill: Option<String>,
    },
    /// Transcribe a meeting's audio with the configured on-device model
    Transcribe {
        /// Meeting id whose audio should be transcribed
        id: String,
    },
    /// Run the read-only Loofah MCP server over stdio
    Mcp,
    /// Browse the vault's tag registry
    Tags {
        #[command(subcommand)]
        command: TagsCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum TagsCommand {
    /// List every tag registered in the vault
    List,
}

#[derive(Debug, Subcommand)]
pub enum MeetingCommand {
    /// List meetings, optionally filtered by text or tags
    List {
        #[arg(short, long)]
        query: Option<String>,
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..=200), help = "Maximum results (1-200)")]
        limit: u32,
        #[arg(long, default_value_t = 0, help = "Number of results to skip")]
        offset: u32,
        #[arg(
            long,
            value_name = "NAME",
            value_parser = parse_tag,
            conflicts_with = "untagged",
            help = "Only meetings carrying this tag; repeatable, all must match"
        )]
        tag: Vec<String>,
        #[arg(long, help = "Only meetings without any tags")]
        untagged: bool,
    },
    /// Search across meeting titles, notes, summaries, and transcripts
    #[command(group = clap::ArgGroup::new("criteria").required(true).multiple(true).args(["query", "speaker"]))]
    Search {
        /// Case-insensitive terms that must all occur
        query: Option<String>,
        #[arg(
            long,
            help = "Person id or name substring; limits hits to meetings where that person spoke"
        )]
        speaker: Option<String>,
        #[arg(
            long,
            value_enum,
            help = "Restrict to a source; repeatable, defaults to all"
        )]
        kind: Vec<SearchKindArg>,
        #[arg(long, default_value_t = DEFAULT_SEARCH_LIMIT, value_parser = clap::value_parser!(u32).range(1..=MAX_SEARCH_LIMIT as i64), help = "Maximum hits (1-50)")]
        limit: u32,
        #[arg(long, default_value_t = 0, help = "Number of hits to skip")]
        offset: u32,
    },
    /// Show meeting metadata, notes, summaries, and action items
    Get { id: String },
    /// Create a meeting note and print its id
    New {
        #[arg(long, help = "Title for the new meeting")]
        title: String,
        #[arg(
            long,
            value_name = "FILE",
            help = "Initial note body read from FILE, or '-' for stdin"
        )]
        note: Option<PathBuf>,
        #[arg(
            long,
            value_name = "RFC3339",
            value_parser = parse_rfc3339_utc,
            help = "Creation timestamp for the meeting (RFC 3339), e.g. 2024-05-01T09:00:00Z; defaults to now"
        )]
        created_at: Option<String>,
        #[arg(
            long,
            value_name = "RFC3339",
            value_parser = parse_rfc3339_utc,
            help = "Start timestamp for the meeting (RFC 3339)"
        )]
        started_at: Option<String>,
        #[arg(
            long,
            value_name = "RFC3339",
            value_parser = parse_rfc3339_utc,
            help = "End timestamp for the meeting (RFC 3339)"
        )]
        ended_at: Option<String>,
        #[arg(
            long,
            value_name = "NAME",
            value_parser = parse_tag,
            help = "Tag for the new meeting; repeatable"
        )]
        tag: Vec<String>,
        #[arg(
            long,
            value_name = "NAME",
            help = "Record who wrote this meeting's note; set this when writing on behalf of someone other than the vault owner (e.g. claude-code)"
        )]
        author: Option<String>,
        #[arg(
            long,
            value_name = "NAME",
            requires = "author",
            help = "Record the skill used to produce this meeting's note (e.g. meeting-summarizer); only meaningful together with --author"
        )]
        skill: Option<String>,
    },
    /// Show the note or generated summaries for a meeting, or edit the note
    Note {
        id: String,
        #[arg(long, value_enum, default_value_t = DocumentKind::Note, conflicts_with_all = ["set", "append"])]
        kind: DocumentKind,
        #[arg(
            long,
            value_name = "FILE",
            help = "Replace the note body with FILE, or '-' for stdin"
        )]
        set: Option<PathBuf>,
        #[arg(
            long,
            value_name = "FILE",
            conflicts_with = "set",
            help = "Append FILE (or '-' for stdin) to the note body"
        )]
        append: Option<PathBuf>,
    },
    /// Show the full speaker-labeled meeting transcript
    Transcript { id: String },
    /// Add or remove tags on a meeting
    Tag {
        #[command(subcommand)]
        command: TagCommand,
    },
    /// Print the absolute path of a meeting's session directory
    Path { id: String },
    /// Store a file as a note attachment of a meeting and print its attachment id
    Attach {
        id: String,
        /// File to store under the meeting's attachments directory
        file: PathBuf,
        #[arg(
            long,
            value_name = "NAME",
            help = "Filename to store the attachment under; defaults to the file's name"
        )]
        name: Option<String>,
    },
    /// Export a meeting to Markdown or JSON
    Export {
        id: String,
        #[arg(long, value_enum, default_value_t = ExportFormat::Markdown)]
        format: ExportFormat,
        #[arg(short, long, value_name = "FILE")]
        output: Option<PathBuf>,
        #[arg(long, requires = "output", help = "Replace an existing output file")]
        force: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum TagCommand {
    /// Add tags to a meeting, registering new ones in the vault's tag registry
    Add {
        id: String,
        #[arg(required = true, value_name = "TAG", value_parser = parse_tag)]
        tags: Vec<String>,
    },
    /// Remove tags from a meeting; the tag registry keeps the names
    Remove {
        id: String,
        #[arg(required = true, value_name = "TAG", value_parser = parse_tag)]
        tags: Vec<String>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum SearchKindArg {
    Title,
    Note,
    Summary,
    Transcript,
}

impl From<SearchKindArg> for SearchKind {
    fn from(kind: SearchKindArg) -> Self {
        match kind {
            SearchKindArg::Title => Self::Title,
            SearchKindArg::Note => Self::Note,
            SearchKindArg::Summary => Self::Summary,
            SearchKindArg::Transcript => Self::Transcript,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum DocumentKind {
    #[default]
    Note,
    Summary,
    All,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum ExportFormat {
    #[default]
    Markdown,
    Json,
}

/// Validates timestamps at argument parsing, before any vault write, and
/// normalizes them to millisecond UTC RFC 3339 — the desktop's
/// `new Date().toISOString()` format that the vault stores everywhere. The
/// stored timestamp remains independent of the immutable session directory.
fn parse_rfc3339_utc(value: &str) -> Result<String, String> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|timestamp| {
            timestamp
                .with_timezone(&chrono::Utc)
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        })
        .map_err(|error| format!("invalid RFC 3339 timestamp: {error}"))
}

fn parse_tag(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("tag name cannot be empty".to_string());
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use clap::CommandFactory;

    #[test]
    fn parses_meeting_list_filters() {
        let args = Args::parse_from([
            "loof", "--json", "meetings", "list", "--query", "planning", "--limit", "10",
        ]);

        assert!(args.json);
        let Command::Sessions { command } = args.command else {
            panic!("expected sessions command");
        };
        let MeetingCommand::List { query, limit, .. } = command else {
            panic!("expected list command");
        };
        assert_eq!(query.as_deref(), Some("planning"));
        assert_eq!(limit, 10);
    }

    #[test]
    fn parses_sessions_command_as_primary_namespace() {
        let Command::Sessions { command } =
            Args::parse_from(["loof", "sessions", "list", "--query", "planning"]).command
        else {
            panic!("expected sessions command");
        };
        let MeetingCommand::List { query, .. } = command else {
            panic!("expected list command");
        };
        assert_eq!(query.as_deref(), Some("planning"));
    }

    #[test]
    fn parses_meeting_list_tag_filters_and_their_conflict() {
        let Command::Sessions { command } = Args::parse_from([
            "loof",
            "meetings",
            "list",
            "--tag",
            " Hiring ",
            "--tag",
            "project-x",
        ])
        .command
        else {
            panic!("expected meetings command");
        };
        let MeetingCommand::List { tag, untagged, .. } = command else {
            panic!("expected list command");
        };
        assert_eq!(tag, vec!["Hiring".to_string(), "project-x".to_string()]);
        assert!(!untagged);

        let Command::Sessions { command } =
            Args::parse_from(["loof", "meetings", "list", "--untagged"]).command
        else {
            panic!("expected meetings command");
        };
        assert!(matches!(
            command,
            MeetingCommand::List { untagged: true, .. }
        ));

        assert!(
            Args::try_parse_from(["loof", "meetings", "list", "--tag", "a", "--untagged"]).is_err()
        );
    }

    #[test]
    fn parses_tag_edit_path_and_tags_list_commands() {
        let Command::Sessions { command } = Args::parse_from([
            "loof",
            "meetings",
            "tag",
            "add",
            "meeting-1",
            "#Hiring",
            "project-x",
        ])
        .command
        else {
            panic!("expected meetings command");
        };
        let MeetingCommand::Tag {
            command: TagCommand::Add { id, tags },
        } = command
        else {
            panic!("expected tag add command");
        };
        assert_eq!(id, "meeting-1");
        assert_eq!(tags, vec!["#Hiring".to_string(), "project-x".to_string()]);

        let Command::Sessions { command } =
            Args::parse_from(["loof", "meetings", "tag", "remove", "meeting-1", "hiring"]).command
        else {
            panic!("expected meetings command");
        };
        assert!(matches!(
            command,
            MeetingCommand::Tag {
                command: TagCommand::Remove { .. }
            }
        ));

        // At least one tag is required, and blank tags are rejected at parse time.
        assert!(Args::try_parse_from(["loof", "meetings", "tag", "add", "meeting-1"]).is_err());
        assert!(
            Args::try_parse_from(["loof", "meetings", "tag", "remove", "meeting-1", "  "]).is_err()
        );

        let Command::Sessions { command } =
            Args::parse_from(["loof", "meetings", "path", "meeting-1"]).command
        else {
            panic!("expected meetings command");
        };
        assert!(matches!(
            command,
            MeetingCommand::Path { id } if id == "meeting-1"
        ));

        assert!(matches!(
            Args::parse_from(["loof", "tags", "list"]).command,
            Command::Tags {
                command: TagsCommand::List
            }
        ));
    }

    #[test]
    fn help_exposes_mcp_and_export() {
        let help = Args::command().render_long_help().to_string();
        assert!(help.contains("sessions"));
        assert!(help.contains("mcp"));
        assert!(help.contains("doctor"));
        assert!(help.contains("AGENTS.md at the vault root"));

        let Command::Sessions { command } = Args::parse_from([
            "loof",
            "meetings",
            "export",
            "meeting-1",
            "--format",
            "json",
        ])
        .command
        else {
            panic!("expected meetings command");
        };
        assert!(matches!(
            command,
            MeetingCommand::Export {
                format: ExportFormat::Json,
                ..
            }
        ));
    }

    #[test]
    fn parses_transcript_command() {
        let Command::Sessions { command } =
            Args::parse_from(["loof", "meetings", "transcript", "meeting-1"]).command
        else {
            panic!("expected meetings command");
        };
        assert!(matches!(
            command,
            MeetingCommand::Transcript { id } if id == "meeting-1"
        ));
    }

    #[test]
    fn parses_search_filters_and_requires_query_or_speaker() {
        let Command::Sessions { command } = Args::parse_from([
            "loof",
            "meetings",
            "search",
            "--speaker",
            "bob",
            "--kind",
            "transcript",
        ])
        .command
        else {
            panic!("expected meetings command");
        };
        let MeetingCommand::Search {
            query,
            speaker,
            kind,
            limit,
            offset,
        } = command
        else {
            panic!("expected search command");
        };
        assert_eq!(query, None);
        assert_eq!(speaker.as_deref(), Some("bob"));
        assert_eq!(kind, vec![SearchKindArg::Transcript]);
        assert_eq!(limit, 20);
        assert_eq!(offset, 0);

        assert!(Args::try_parse_from(["loof", "meetings", "search"]).is_err());
    }

    #[test]
    fn parses_new_command_with_note_source() {
        let Command::Sessions { command } = Args::parse_from([
            "loof", "meetings", "new", "--title", "Planning", "--note", "-",
        ])
        .command
        else {
            panic!("expected meetings command");
        };
        let MeetingCommand::New { title, note, .. } = command else {
            panic!("expected new command");
        };
        assert_eq!(title, "Planning");
        assert_eq!(note.as_deref(), Some(Path::new("-")));

        assert!(Args::try_parse_from(["loof", "meetings", "new"]).is_err());
    }

    #[test]
    fn parses_new_timestamps_normalized_to_utc_and_repeatable_tags() {
        let Command::Sessions { command } = Args::parse_from([
            "loof",
            "meetings",
            "new",
            "--title",
            "Retro",
            "--created-at",
            "2024-05-01T09:00:00+02:00",
            "--started-at",
            "2024-05-01T09:05:00Z",
            "--ended-at",
            "2024-05-01T10:00:00.250Z",
            "--tag",
            " project-x ",
            "--tag",
            "retro",
        ])
        .command
        else {
            panic!("expected meetings command");
        };
        let MeetingCommand::New {
            created_at,
            started_at,
            ended_at,
            tag,
            ..
        } = command
        else {
            panic!("expected new command");
        };
        // Offsets collapse to UTC millisecond precision at parse time.
        assert_eq!(created_at.as_deref(), Some("2024-05-01T07:00:00.000Z"));
        assert_eq!(started_at.as_deref(), Some("2024-05-01T09:05:00.000Z"));
        assert_eq!(ended_at.as_deref(), Some("2024-05-01T10:00:00.250Z"));
        // Tags are trimmed but otherwise kept verbatim.
        assert_eq!(tag, vec!["project-x".to_string(), "retro".to_string()]);
    }

    #[test]
    fn rejects_invalid_timestamps_and_empty_tags_at_parse_time() {
        assert!(
            Args::try_parse_from([
                "loof",
                "meetings",
                "new",
                "--title",
                "Retro",
                "--created-at",
                "yesterday",
            ])
            .is_err()
        );
        assert!(
            Args::try_parse_from([
                "loof",
                "meetings",
                "new",
                "--title",
                "Retro",
                "--started-at",
                "2024-05-01",
            ])
            .is_err()
        );
        assert!(
            Args::try_parse_from(["loof", "meetings", "new", "--title", "Retro", "--tag", "  "])
                .is_err()
        );
    }

    #[test]
    fn note_edit_flags_are_mutually_exclusive_and_conflict_with_kind() {
        assert!(
            Args::try_parse_from([
                "loof",
                "meetings",
                "note",
                "meeting-1",
                "--set",
                "a.md",
                "--append",
                "b.md",
            ])
            .is_err()
        );
        assert!(
            Args::try_parse_from([
                "loof",
                "meetings",
                "note",
                "meeting-1",
                "--kind",
                "summary",
                "--set",
                "a.md",
            ])
            .is_err()
        );

        let Command::Sessions { command } = Args::parse_from([
            "loof",
            "meetings",
            "note",
            "meeting-1",
            "--append",
            "extra.md",
        ])
        .command
        else {
            panic!("expected meetings command");
        };
        assert!(matches!(
            command,
            MeetingCommand::Note {
                set: None,
                append: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn parses_attach_with_and_without_name_override() {
        let Command::Sessions { command } =
            Args::parse_from(["loof", "meetings", "attach", "meeting-1", "diagram.png"]).command
        else {
            panic!("expected meetings command");
        };
        let MeetingCommand::Attach { id, file, name } = command else {
            panic!("expected attach command");
        };
        assert_eq!(id, "meeting-1");
        assert_eq!(file, Path::new("diagram.png"));
        assert_eq!(name, None);

        let Command::Sessions { command } = Args::parse_from([
            "loof",
            "meetings",
            "attach",
            "meeting-1",
            "/tmp/export.pdf",
            "--name",
            "Q1 report.pdf",
        ])
        .command
        else {
            panic!("expected meetings command");
        };
        let MeetingCommand::Attach { name, .. } = command else {
            panic!("expected attach command");
        };
        assert_eq!(name.as_deref(), Some("Q1 report.pdf"));

        assert!(Args::try_parse_from(["loof", "meetings", "attach", "meeting-1"]).is_err());
    }

    #[test]
    fn parses_import_command_with_optional_title() {
        let Command::Import {
            file,
            title,
            into,
            transcribe,
            ..
        } = Args::parse_from(["loof", "import", "meeting.m4a"]).command
        else {
            panic!("expected import command");
        };
        assert_eq!(file, Path::new("meeting.m4a"));
        assert_eq!(title, None);
        assert_eq!(into, None);
        assert!(!transcribe);

        let Command::Import { title, .. } =
            Args::parse_from(["loof", "import", "meeting.m4a", "--title", "Weekly sync"]).command
        else {
            panic!("expected import command");
        };
        assert_eq!(title.as_deref(), Some("Weekly sync"));

        assert!(Args::try_parse_from(["loof", "import"]).is_err());
    }

    #[test]
    fn parses_import_into_and_rejects_combining_it_with_title() {
        let Command::Import { into, .. } =
            Args::parse_from(["loof", "import", "meeting.m4a", "--into", "meeting-1"]).command
        else {
            panic!("expected import command");
        };
        assert_eq!(into.as_deref(), Some("meeting-1"));

        // The target meeting already has a title; the two flags are exclusive.
        assert!(
            Args::try_parse_from([
                "loof",
                "import",
                "meeting.m4a",
                "--into",
                "meeting-1",
                "--title",
                "Weekly sync",
            ])
            .is_err()
        );
    }

    #[test]
    fn import_timestamps_only_apply_to_new_meetings() {
        let Command::Import { created_at, .. } = Args::parse_from([
            "loof",
            "import",
            "meeting.m4a",
            "--created-at",
            "2024-05-01T09:00:00Z",
        ])
        .command
        else {
            panic!("expected import command");
        };
        assert_eq!(created_at.as_deref(), Some("2024-05-01T09:00:00.000Z"));

        // An existing meeting keeps its own timestamps; the flags are exclusive
        // with --into.
        for flag in ["--created-at", "--started-at", "--ended-at"] {
            assert!(
                Args::try_parse_from([
                    "loof",
                    "import",
                    "meeting.m4a",
                    "--into",
                    "meeting-1",
                    flag,
                    "2024-05-01T09:00:00Z",
                ])
                .is_err()
            );
        }
    }

    #[test]
    fn parses_import_transcribe_flag_and_transcribe_command() {
        let Command::Import { transcribe, .. } =
            Args::parse_from(["loof", "import", "meeting.m4a", "--transcribe"]).command
        else {
            panic!("expected import command");
        };
        assert!(transcribe);

        let Command::Transcribe { id } =
            Args::parse_from(["loof", "transcribe", "meeting-1"]).command
        else {
            panic!("expected transcribe command");
        };
        assert_eq!(id, "meeting-1");

        assert!(Args::try_parse_from(["loof", "transcribe"]).is_err());
    }

    #[test]
    fn export_force_requires_an_output_path() {
        assert!(
            Args::try_parse_from(["loof", "meetings", "export", "meeting-1", "--force"]).is_err()
        );
    }

    #[test]
    fn public_docs_and_skill_cover_the_command_contract() {
        let docs = include_str!("../../../docs/src/content/docs/reference/cli.mdx");
        let skill = concat!(
            include_str!("../../../skills/loofah/references/cli.md"),
            include_str!("../../../skills/loofah/references/setup.md"),
        );
        let command = Args::command();
        let mut paths = Vec::new();
        collect_leaf_commands(&command, "", &mut paths);

        let vault_agents = include_str!("../../../docs/src/content/docs/agents/vault.md");

        for path in paths {
            assert!(docs.contains(&path), "CLI docs are missing `{path}`");
            assert!(skill.contains(&path), "loofah skill is missing `{path}`");
            assert!(
                vault_agents.contains(&path),
                "vault AGENTS.md page is missing `{path}`"
            );
        }
        assert_options_are_documented(&command, docs);
    }

    #[test]
    fn cli_contract_matches_snapshot() {
        let contract: serde_json::Value =
            serde_json::from_str(&cli_docs::generate_json(&Args::command())).unwrap();
        insta::assert_json_snapshot!("cli_contract", canonicalize_json(contract));
    }

    fn canonicalize_json(value: serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(object) => serde_json::Value::Object(
                object
                    .into_iter()
                    .map(|(key, value)| (key, canonicalize_json(value)))
                    .collect::<std::collections::BTreeMap<_, _>>()
                    .into_iter()
                    .collect(),
            ),
            serde_json::Value::Array(values) => {
                serde_json::Value::Array(values.into_iter().map(canonicalize_json).collect())
            }
            value => value,
        }
    }

    fn collect_leaf_commands(command: &clap::Command, prefix: &str, paths: &mut Vec<String>) {
        for subcommand in command
            .get_subcommands()
            .filter(|subcommand| subcommand.get_name() != "help")
        {
            let path = if prefix.is_empty() {
                subcommand.get_name().to_string()
            } else {
                format!("{prefix} {}", subcommand.get_name())
            };
            if subcommand
                .get_subcommands()
                .any(|child| child.get_name() != "help")
            {
                collect_leaf_commands(subcommand, &path, paths);
            } else {
                paths.push(path);
            }
        }
    }

    fn assert_options_are_documented(command: &clap::Command, docs: &str) {
        for argument in command.get_arguments() {
            if let Some(long) = argument.get_long() {
                assert!(
                    docs.contains(&format!("--{long}")),
                    "CLI docs are missing `--{long}`"
                );
            }
        }
        for subcommand in command.get_subcommands() {
            assert_options_are_documented(subcommand, docs);
        }
    }
}
