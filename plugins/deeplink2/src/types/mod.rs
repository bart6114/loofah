mod auth_callback;
mod billing_refresh;
mod share_open;

pub use auth_callback::*;
pub use billing_refresh::*;
pub use share_open::*;

use serde::{Deserialize, Serialize};
use specta::Type;
use std::str::FromStr;

const SHARE_OPEN_PREFIXES: [&str; 4] = [
    "loofah://share/open",
    "loofah-staging://share/open",
    "freemeetingtranscriber://share/open",
    "freemeetingtranscriber-staging://share/open",
];
const MAX_SHARE_OPEN_URL_BYTES: usize = 512;

#[derive(Debug, Clone, serde::Serialize, specta::Type, tauri_specta::Event)]
pub struct DeepLinkEvent(pub DeepLink);

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(tag = "to", content = "search")]
pub enum DeepLink {
    #[serde(rename = "/auth/callback")]
    AuthCallback(AuthCallbackSearch),
    #[serde(rename = "/billing/refresh")]
    BillingRefresh(BillingRefreshSearch),
    #[serde(rename = "/meetings/open")]
    OpenMeeting { id: String },
}

pub(crate) enum IncomingDeepLink {
    Existing(DeepLink),
    ShareOpen(ShareOpenRequest),
}

impl FromStr for IncomingDeepLink {
    type Err = crate::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let candidate = s.trim_matches(|character: char| character.is_ascii_whitespace());
        if candidate.len() > MAX_SHARE_OPEN_URL_BYTES
            && SHARE_OPEN_PREFIXES.iter().any(|expected| {
                candidate
                    .get(..expected.len())
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case(expected))
            })
        {
            return Err(crate::Error::InvalidShareOpen);
        }

        let parsed = url::Url::parse(candidate)?;
        let host = parsed.host_str().unwrap_or("");
        let path = parsed.path().trim_start_matches('/');
        let full_path = if path.is_empty() {
            host.to_string()
        } else {
            format!("{host}/{path}")
        };

        if full_path == "share/open" {
            return ShareOpenRequest::parse(&parsed).map(Self::ShareOpen);
        }

        DeepLink::from_str(candidate).map(Self::Existing)
    }
}

#[cfg(test)]
mod incoming_tests {
    use super::*;

    #[test]
    fn rejects_oversized_share_open_before_url_parsing() {
        for prefix in SHARE_OPEN_PREFIXES {
            let value = format!("{prefix}?{}", "unknown=x&".repeat(64));
            assert!(value.len() > MAX_SHARE_OPEN_URL_BYTES);
            assert!(matches!(
                IncomingDeepLink::from_str(&value),
                Err(crate::Error::InvalidShareOpen)
            ));
            assert!(matches!(
                IncomingDeepLink::from_str(&format!(" \n{value}")),
                Err(crate::Error::InvalidShareOpen)
            ));
        }
    }
}

impl DeepLink {
    pub fn path(&self) -> &'static str {
        match self {
            DeepLink::AuthCallback(_) => "/auth/callback",
            DeepLink::BillingRefresh(_) => "/billing/refresh",
            DeepLink::OpenMeeting { .. } => "/meetings/open",
        }
    }
}

impl FromStr for DeepLink {
    type Err = crate::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parsed = url::Url::parse(s)?;

        let host = parsed.host_str().unwrap_or("");
        #[cfg(target_os = "macos")]
        if parsed.scheme() == "loofah" && host == "meetings" {
            return parse_meeting_link(&parsed);
        }
        let path = parsed.path().trim_start_matches('/');
        let full_path = if path.is_empty() {
            host.to_string()
        } else {
            format!("{}/{}", host, path)
        };

        let query = parsed.query().unwrap_or("");

        match full_path.as_str() {
            "auth/callback" => Ok(DeepLink::AuthCallback(serde_qs::from_str(query)?)),
            "billing/refresh" => Ok(DeepLink::BillingRefresh(serde_qs::from_str(query)?)),
            _ => Err(crate::Error::UnknownPath(full_path)),
        }
    }
}

#[cfg(target_os = "macos")]
fn parse_meeting_link(url: &url::Url) -> Result<DeepLink, crate::Error> {
    let invalid = || crate::Error::InvalidUrl("meeting link".into());
    let mut segments = url.path_segments().ok_or_else(invalid)?;
    let id = segments.next().ok_or_else(invalid)?;
    if segments.next().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid());
    }
    let id = percent_encoding::percent_decode_str(id)
        .decode_utf8()
        .map_err(|_| invalid())?;
    hypr_vault_read::paths::validate_session_id(&id).map_err(|_| invalid())?;
    Ok(DeepLink::OpenMeeting {
        id: id.into_owned(),
    })
}

#[cfg(all(test, target_os = "macos"))]
mod meeting_tests {
    use super::*;

    #[test]
    fn opens_a_meeting_by_session_id() {
        for (url, expected) in [
            ("loofah://meetings/meeting-1", "meeting-1"),
            ("loofah://meetings/Team%20review", "Team review"),
        ] {
            assert!(matches!(
                IncomingDeepLink::from_str(url),
                Ok(IncomingDeepLink::Existing(DeepLink::OpenMeeting { id })) if id == expected
            ));
        }
    }

    #[test]
    fn rejects_invalid_meeting_links() {
        for url in [
            "loofah://meetings",
            "loofah://meetings/",
            "loofah://meetings/meeting-1/transcript",
            "loofah://meetings/meeting-1/",
            "loofah://meetings/one%2Ftwo",
            "loofah://meetings/%FF",
            "loofah://meetings/meeting-1?t=8",
            "loofah://meetings/meeting-1#note",
            "loofah://user@meetings/meeting-1",
            "https://meetings/meeting-1",
        ] {
            assert!(IncomingDeepLink::from_str(url).is_err(), "{url}");
        }
    }
}
