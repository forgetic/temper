use super::{ForgeError, ForgeResult, ItemListDetails};
use serde::{Deserialize, Deserializer, Serialize};

/// Lifecycle bucket used by consolidated candidate discovery.
///
/// `Terminal` means closed issues and both closed and merged pull requests.
#[derive(Copy, Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateLifecycle {
    Open,
    Terminal,
}

/// Descriptive alias for [`CandidateLifecycle`].
pub type CandidateLifecycleBucket = CandidateLifecycle;

/// Label selection used by consolidated candidate discovery.
///
/// Unlike the conjunctive `labels` fields on [`super::IssueQuery`] and
/// [`super::PullRequestQuery`], `AnyOf` is disjunctive. Candidate APIs reject an
/// empty `AnyOf`; use [`Self::Unfiltered`] for a lifecycle-bounded unlabelled
/// read.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateLabelSelection {
    Unfiltered,
    AnyOf(Vec<String>),
}

impl<'de> Deserialize<'de> for CandidateLabelSelection {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "snake_case")]
        enum Repr {
            Unfiltered,
            AnyOf(Vec<String>),
        }

        match Repr::deserialize(deserializer)? {
            Repr::Unfiltered => Ok(Self::Unfiltered),
            Repr::AnyOf(labels) => Self::any_of(labels).map_err(serde::de::Error::custom),
        }
    }
}

/// Concise alias for [`CandidateLabelSelection`].
pub type CandidateLabels = CandidateLabelSelection;

impl CandidateLabelSelection {
    /// Builds a normalized non-empty any-label selection.
    pub fn any_of<I, S>(labels: I) -> ForgeResult<Self>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let labels = normalize_candidate_labels(labels.into_iter().map(Into::into).collect())?;
        Ok(Self::AnyOf(labels))
    }

    /// Returns a validated, sorted, deduplicated selection for backend adapters.
    pub fn normalized(&self) -> ForgeResult<Option<Vec<String>>> {
        match self {
            Self::Unfiltered => Ok(None),
            Self::AnyOf(labels) => Ok(Some(normalize_candidate_labels(labels.clone())?)),
        }
    }
}

impl Default for CandidateLabelSelection {
    fn default() -> Self {
        Self::Unfiltered
    }
}

fn normalize_candidate_labels(mut labels: Vec<String>) -> ForgeResult<Vec<String>> {
    if labels.iter().any(String::is_empty) {
        return Err(ForgeError::InvalidRequest(
            "candidate labels must not be empty strings".to_string(),
        ));
    }
    labels.sort();
    labels.dedup();
    if labels.is_empty() {
        return Err(ForgeError::InvalidRequest(
            "candidate AnyOf labels must be non-empty".to_string(),
        ));
    }
    Ok(labels)
}

/// Consolidated issue candidate query.
///
/// Candidate queries default to an unfiltered open lifecycle bucket and
/// summary detail. Runtime planners use `Unfiltered` only for open default-kind
/// intake; terminal planning always supplies bounded `AnyOf` interest.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct IssueCandidateQuery {
    pub lifecycle: CandidateLifecycle,
    pub labels: CandidateLabelSelection,
    #[serde(default = "ItemListDetails::summary")]
    pub details: ItemListDetails,
}

impl Default for IssueCandidateQuery {
    fn default() -> Self {
        Self {
            lifecycle: CandidateLifecycle::Open,
            labels: CandidateLabelSelection::Unfiltered,
            details: ItemListDetails::summary(),
        }
    }
}

/// Consolidated pull-request candidate query.
///
/// `Terminal` covers closed and merged pull requests. Defaults otherwise match
/// [`IssueCandidateQuery`].
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct PullRequestCandidateQuery {
    pub lifecycle: CandidateLifecycle,
    pub labels: CandidateLabelSelection,
    #[serde(default = "ItemListDetails::summary")]
    pub details: ItemListDetails,
}

impl Default for PullRequestCandidateQuery {
    fn default() -> Self {
        Self {
            lifecycle: CandidateLifecycle::Open,
            labels: CandidateLabelSelection::Unfiltered,
            details: ItemListDetails::summary(),
        }
    }
}
