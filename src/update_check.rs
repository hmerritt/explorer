use std::{fmt, sync::Arc};

use futures::AsyncReadExt;
use gpui::http_client::{HttpClient, http::StatusCode};
use semver::Version;
use serde::Deserialize;

pub(crate) const LATEST_RELEASE_URL: &str =
    "https://api.github.com/repos/hmerritt/explorer/releases/latest";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum UpdateCheckResult {
    UpdateAvailable {
        current: Version,
        latest: Version,
        release_url: String,
    },
    UpToDate {
        current: Version,
        latest: Version,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum UpdateCheckError {
    Network(String),
    RateLimited,
    MissingRelease,
    InvalidResponse(String),
    Timeout,
}

impl fmt::Display for UpdateCheckError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Network(error) => write!(formatter, "Network request failed: {error}"),
            Self::RateLimited => formatter
                .write_str("GitHub's request limit was reached. Wait a little, then try again."),
            Self::MissingRelease => {
                formatter.write_str("No published stable Explorer release was found.")
            }
            Self::InvalidResponse(error) => {
                write!(formatter, "GitHub returned an invalid response: {error}")
            }
            Self::Timeout => formatter.write_str("The update check timed out after 15 seconds."),
        }
    }
}

#[derive(Deserialize)]
struct LatestReleaseResponse {
    tag_name: String,
    html_url: String,
    draft: bool,
    prerelease: bool,
}

fn parse_release_version(tag: &str) -> Result<Version, UpdateCheckError> {
    let version = tag.strip_prefix('v').unwrap_or(tag);
    Version::parse(version).map_err(|error| {
        UpdateCheckError::InvalidResponse(format!(
            "release tag {tag:?} is not semantic version: {error}"
        ))
    })
}

pub(crate) fn evaluate_latest_release_response(
    current_version: &str,
    status: StatusCode,
    body: &[u8],
) -> Result<UpdateCheckResult, UpdateCheckError> {
    if status == StatusCode::FORBIDDEN || status == StatusCode::TOO_MANY_REQUESTS {
        return Err(UpdateCheckError::RateLimited);
    }
    if status == StatusCode::NOT_FOUND {
        return Err(UpdateCheckError::MissingRelease);
    }
    if !status.is_success() {
        return Err(UpdateCheckError::InvalidResponse(format!("HTTP {status}")));
    }

    let response: LatestReleaseResponse = serde_json::from_slice(body)
        .map_err(|error| UpdateCheckError::InvalidResponse(error.to_string()))?;
    if response.draft || response.prerelease {
        return Err(UpdateCheckError::InvalidResponse(
            "the latest-release endpoint returned an unpublished or prerelease build".into(),
        ));
    }
    if !response
        .html_url
        .starts_with("https://github.com/hmerritt/explorer/releases/")
    {
        return Err(UpdateCheckError::InvalidResponse(
            "the release page URL was missing or unexpected".into(),
        ));
    }

    let current = Version::parse(current_version).map_err(|error| {
        UpdateCheckError::InvalidResponse(format!("compiled version is invalid: {error}"))
    })?;
    let latest = parse_release_version(&response.tag_name)?;
    if latest > current {
        Ok(UpdateCheckResult::UpdateAvailable {
            current,
            latest,
            release_url: response.html_url,
        })
    } else {
        Ok(UpdateCheckResult::UpToDate { current, latest })
    }
}

pub(crate) async fn check_latest_release(
    client: Arc<dyn HttpClient>,
) -> Result<UpdateCheckResult, UpdateCheckError> {
    let mut response = client
        .get(LATEST_RELEASE_URL, ().into(), true)
        .await
        .map_err(|error| UpdateCheckError::Network(error.to_string()))?;
    let status = response.status();
    let mut body = Vec::new();
    response
        .body_mut()
        .read_to_end(&mut body)
        .await
        .map_err(|error| UpdateCheckError::Network(error.to_string()))?;
    evaluate_latest_release_response(env!("CARGO_PKG_VERSION"), status, &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(tag: &str, url: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "tag_name": tag,
            "html_url": url,
            "draft": false,
            "prerelease": false
        }))
        .unwrap()
    }

    #[test]
    fn optional_v_prefix_and_newer_release_produce_update_available() {
        let result = evaluate_latest_release_response(
            "1.2.3",
            StatusCode::OK,
            &fixture(
                "v1.3.0",
                "https://github.com/hmerritt/explorer/releases/tag/v1.3.0",
            ),
        )
        .unwrap();
        assert!(matches!(
            result,
            UpdateCheckResult::UpdateAvailable { current, latest, .. }
                if current == Version::new(1, 2, 3) && latest == Version::new(1, 3, 0)
        ));
    }

    #[test]
    fn equal_or_older_release_never_suggests_a_downgrade() {
        for tag in ["1.2.3", "v1.1.9"] {
            let result = evaluate_latest_release_response(
                "1.2.3",
                StatusCode::OK,
                &fixture(
                    tag,
                    "https://github.com/hmerritt/explorer/releases/tag/v1.2.3",
                ),
            )
            .unwrap();
            assert!(matches!(result, UpdateCheckResult::UpToDate { .. }));
        }
    }

    #[test]
    fn response_errors_are_specific_and_retryable() {
        assert_eq!(
            evaluate_latest_release_response("1.0.0", StatusCode::FORBIDDEN, b"{}"),
            Err(UpdateCheckError::RateLimited)
        );
        assert_eq!(
            evaluate_latest_release_response("1.0.0", StatusCode::NOT_FOUND, b"{}"),
            Err(UpdateCheckError::MissingRelease)
        );
        assert!(matches!(
            evaluate_latest_release_response("1.0.0", StatusCode::OK, b"not json"),
            Err(UpdateCheckError::InvalidResponse(_))
        ));
        assert!(matches!(
            evaluate_latest_release_response(
                "1.0.0",
                StatusCode::OK,
                &fixture(
                    "nightly",
                    "https://github.com/hmerritt/explorer/releases/tag/nightly"
                )
            ),
            Err(UpdateCheckError::InvalidResponse(_))
        ));
    }

    #[test]
    fn prerelease_payload_is_rejected_even_if_the_endpoint_misbehaves() {
        let body = serde_json::to_vec(&serde_json::json!({
            "tag_name": "v2.0.0-beta.1",
            "html_url": "https://github.com/hmerritt/explorer/releases/tag/v2.0.0-beta.1",
            "draft": false,
            "prerelease": true
        }))
        .unwrap();
        assert!(matches!(
            evaluate_latest_release_response("1.0.0", StatusCode::OK, &body),
            Err(UpdateCheckError::InvalidResponse(_))
        ));
    }

    #[test]
    fn transient_error_states_have_actionable_messages() {
        let network = UpdateCheckError::Network("offline fixture".into()).to_string();
        let timeout = UpdateCheckError::Timeout.to_string();
        assert!(network.contains("Network request failed"));
        assert!(timeout.contains("15 seconds"));
    }
}
