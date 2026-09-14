use crate::error::Error;

#[tauri::command]
#[specta::specta]
pub async fn github_issue_state(
    owner: String,
    repo: String,
    number: u64,
) -> Result<crate::github_state::GitHubIssueState, Error> {
    crate::github_state::fetch_public(&owner, &repo, number).await
}

#[tauri::command]
#[specta::specta]
pub async fn github_issue_detail(
    owner: String,
    repo: String,
    number: u64,
) -> Result<hypr_github_issues::Issue, Error> {
    crate::github_state::fetch_issue_detail(&owner, &repo, number).await
}

#[tauri::command]
#[specta::specta]
pub async fn github_issue_comments(
    owner: String,
    repo: String,
    number: u64,
) -> Result<Vec<hypr_github_issues::IssueComment>, Error> {
    crate::github_state::fetch_issue_comments(&owner, &repo, number).await
}
