//! Issue and pull-request operations: comment bodies, PR file listings, and
//! comment creation.

use serde_json::{Value, json};

use super::client::{Auth, Client, RestError, Result, json_ok};

pub async fn list_pull_request_files(
    client: &Client,
    base: &str,
    token: &str,
    owner: &str,
    name: &str,
    number: u64,
) -> Result<Vec<String>> {
    let mut files = Vec::new();
    let mut page = 1;
    loop {
        let resp = client
            .send(
                "GET",
                format!(
                    "{base}/api/v1/repos/{owner}/{name}/pulls/{number}/files?page={page}&limit=50"
                ),
                Auth::Token(token),
                None,
            )
            .await?;
        let payload: Value = json_ok(resp, "list pull request files")?;
        let page_files = payload.as_array().ok_or_else(|| RestError::Shape {
            what: "pull request files".into(),
            detail: "response was not an array".into(),
        })?;
        for file in page_files {
            let filename = file["filename"]
                .as_str()
                .or_else(|| file["name"].as_str())
                .ok_or_else(|| RestError::Shape {
                    what: "pull request file".into(),
                    detail: "file entry had no filename".into(),
                })?;
            files.push(filename.to_string());
        }
        if page_files.len() < 50 {
            return Ok(files);
        }
        page += 1;
    }
}

pub async fn list_issue_comment_bodies(
    client: &Client,
    base: &str,
    token: &str,
    owner: &str,
    name: &str,
    number: u64,
) -> Result<Vec<String>> {
    let mut comments = Vec::new();
    let mut page = 1;
    loop {
        let resp = client
            .send(
                "GET",
                format!(
                    "{base}/api/v1/repos/{owner}/{name}/issues/{number}/comments?page={page}&limit=50"
                ),
                Auth::Token(token),
                None,
            )
            .await?;
        let payload: Value = json_ok(resp, "list issue comments")?;
        let page_comments = payload.as_array().ok_or_else(|| RestError::Shape {
            what: "issue comments".into(),
            detail: "response was not an array".into(),
        })?;
        for comment in page_comments {
            let body = comment["body"].as_str().ok_or_else(|| RestError::Shape {
                what: "issue comment".into(),
                detail: "comment entry had no body".into(),
            })?;
            comments.push(body.to_string());
        }
        if page_comments.len() < 50 {
            return Ok(comments);
        }
        page += 1;
    }
}

pub async fn create_issue_comment(
    client: &Client,
    base: &str,
    token: &str,
    owner: &str,
    name: &str,
    number: u64,
    body: &str,
) -> Result<()> {
    let resp = client
        .send(
            "POST",
            format!("{base}/api/v1/repos/{owner}/{name}/issues/{number}/comments"),
            Auth::Token(token),
            Some(&json!({ "body": body })),
        )
        .await?;
    json_ok(resp, "create issue comment").map(|_| ())
}
