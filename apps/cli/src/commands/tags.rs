use crate::cli::TagsCommand;
use crate::{Error, Result, output};

pub async fn run(cache: &hypr_search_cache::Cache, command: TagsCommand, json: bool) -> Result<()> {
    match command {
        TagsCommand::List => {
            let cache = cache.clone();
            let tags = tokio::task::spawn_blocking(move || cache.tags_fresh())
                .await
                .map_err(|error| Error::operation("list tags", error.to_string()))??;
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
