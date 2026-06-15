//! Mappings for the simple, non-issue/PR artifacts: users, repositories,
//! labels, and comments.

use super::non_empty;
use crate::ids::{
    RepoCoord, format_comment_id, format_label_id, format_repository_id, format_user_id,
};
use crate::types::{CommentDto, LabelDto, RepositoryDto, UserDto};
use temper_forge_model::{Comment, Label, Repository, User};

/// Maps a GitHub user DTO into a portable [`User`].
///
/// The GitHub login is both the portable [`UserId`](temper_forge_model::UserId) and
/// the human-facing handle. `name`/`email` are `null` when unset and map to
/// `None`.
pub(crate) fn map_user(dto: UserDto) -> User {
    User {
        id: format_user_id(&dto.login),
        handle: dto.login,
        display_name: non_empty(dto.name),
        email: non_empty(dto.email),
    }
}

/// Maps a GitHub repository DTO into a portable [`Repository`].
pub(crate) fn map_repository(dto: RepositoryDto) -> Repository {
    let repo = RepoCoord::new(dto.owner.login, dto.name);
    Repository {
        id: format_repository_id(&repo),
        owner: repo.owner,
        name: repo.name,
        default_branch: dto.default_branch,
        description: non_empty(dto.description),
        created_at: dto.created_at,
        updated_at: dto.updated_at,
    }
}

/// Maps a GitHub label DTO into a portable [`Label`] scoped to `repo`.
///
/// The numeric provider id becomes the prefixed opaque
/// [`LabelId`](temper_forge_model::LabelId); empty color/description strings map to
/// `None`. GitHub colors carry no `#` prefix and are passed through unchanged.
pub(crate) fn map_label(repo: &RepoCoord, dto: LabelDto) -> Label {
    Label {
        id: format_label_id(repo, dto.id),
        repo_id: format_repository_id(repo),
        name: dto.name,
        color: non_empty(dto.color),
        description: non_empty(dto.description),
    }
}

/// Maps a GitHub comment DTO into a portable [`Comment`].
pub(crate) fn map_comment(repo: &RepoCoord, dto: CommentDto) -> Comment {
    Comment {
        id: format_comment_id(repo, dto.id),
        author_id: format_user_id(&dto.user.login),
        body: dto.body.unwrap_or_default(),
        created_at: dto.created_at,
        updated_at: dto.updated_at,
    }
}
