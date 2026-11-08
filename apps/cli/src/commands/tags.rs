use crate::cli::TagsCommand;
use crate::{Result, output};

pub async fn run(cache: &hypr_search_cache::Cache, command: TagsCommand, json: bool) -> Result<()> {
    match command {
        TagsCommand::List => {
            let gateway =
                crate::local_owner::LocalGateway::new(cache.vault().to_owned(), cache.clone());
            let tags = match gateway
                .execute(hypr_loofah_protocol::LocalCliOperation::Tags {})
                .await?
                .response
                .result?
            {
                hypr_loofah_protocol::LocalCliReply::Tags { tags } => tags,
                _ => return Err(crate::profiles::unexpected()),
            };
            let rendered = if json {
                output::json("tags.list", &tags, None)?
            } else if tags.is_empty() {
                "No tags found.".to_string()
            } else {
                tags.iter()
                    .map(|tag| tag.name.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            output::emit(&rendered);
            Ok(())
        }
    }
}
