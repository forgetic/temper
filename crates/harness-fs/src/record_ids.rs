use harness_forge::{
    CommentId, ForgeError, ForgeResult, IssueId, ItemNumber, LabelId, PullRequestId, RepositoryId,
};

pub(crate) fn is_record_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

pub(crate) fn issue_id(repo_id: &RepositoryId, number: ItemNumber) -> IssueId {
    IssueId::new(format!("issue-{}-{:016}", repo_id.as_str(), number.get()))
}

pub(crate) fn pull_request_id(repo_id: &RepositoryId, number: ItemNumber) -> PullRequestId {
    PullRequestId::new(format!(
        "pull-request-{}-{:016}",
        repo_id.as_str(),
        number.get()
    ))
}

pub(crate) fn comment_id(issue_id: &IssueId, number: u64) -> CommentId {
    CommentId::new(format!("comment-{}-{number:016}", issue_id.as_str()))
}

pub(crate) fn stored_comment_number(
    issue_id: &IssueId,
    comment_id: &CommentId,
) -> ForgeResult<u64> {
    let prefix = format!("comment-{}-", issue_id.as_str());
    let suffix = comment_id.as_str().strip_prefix(&prefix).ok_or_else(|| {
        ForgeError::Backend(format!(
            "comment {comment_id} on issue {issue_id} does not have deterministic id prefix {prefix}"
        ))
    })?;

    if suffix.len() != 16 || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ForgeError::Backend(format!(
            "comment {comment_id} on issue {issue_id} does not have a 16-digit deterministic number"
        )));
    }

    suffix.parse::<u64>().map_err(|error| {
        ForgeError::Backend(format!(
            "parse deterministic comment number for {comment_id} on issue {issue_id}: {error}"
        ))
    })
}

pub(crate) fn label_id(repo_id: &RepositoryId, label_name: &str) -> LabelId {
    LabelId::new(format!(
        "label-{}-{}",
        repo_id.as_str(),
        hex_encode(label_name.as_bytes())
    ))
}

pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX_DIGITS[(byte >> 4) as usize] as char);
        encoded.push(HEX_DIGITS[(byte & 0x0f) as usize] as char);
    }
    encoded
}
