use std::borrow::Cow;
use std::collections::HashMap;

use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use serde::de::IgnoredAny;

use super::builder::{CLIENT_ID, GQL_URL, URL_REGEX};
use crate::extractor::batch_status::{BatchPresence, BatchStatusProbe};
use crate::extractor::default::DEFAULT_UA;
use crate::extractor::error::ExtractorError;
use crate::extractor::utils::capture_group_1;

/// Twitch's batch status probe, registered with the Twitch platform.
pub(crate) static BATCH_PROBE: TwitchStatusProbe = TwitchStatusProbe {
    endpoint: Cow::Borrowed(GQL_URL),
};

/// Twitch answered for 1000 logins in one query; the cap keeps a request well inside what was seen
/// to work.
const MAX_LOGINS: usize = 100;

/// Fetches only each user's login and whether it has a stream, for a list of logins.
const USERS_QUERY: &str = "query($l:[String!]){users(logins:$l){login stream{id}}}";

/// Live status of Twitch channels from one GraphQL `users(logins: …)` query. Keys are lowercased
/// logins.
pub(crate) struct TwitchStatusProbe {
    endpoint: Cow<'static, str>,
}

#[derive(Deserialize)]
struct UsersResponse {
    data: Option<UsersData>,
    #[serde(default)]
    errors: Vec<GqlError>,
}

#[derive(Deserialize)]
struct UsersData {
    /// One entry per requested login, `null` for a login with no user.
    users: Option<Vec<Option<BatchUser>>>,
}

#[derive(Deserialize)]
struct BatchUser {
    login: String,
    /// `null` while the channel is offline.
    stream: Option<IgnoredAny>,
}

#[derive(Deserialize)]
struct GqlError {
    message: String,
}

#[async_trait]
impl BatchStatusProbe for TwitchStatusProbe {
    fn max_keys(&self) -> usize {
        MAX_LOGINS
    }

    fn key_for_url(&self, url: &str) -> Option<String> {
        capture_group_1(&URL_REGEX, url).map(str::to_ascii_lowercase)
    }

    async fn probe(
        &self,
        client: &Client,
        keys: &[String],
    ) -> Result<HashMap<String, BatchPresence>, ExtractorError> {
        let body = serde_json::json!({ "query": USERS_QUERY, "variables": { "l": keys } });
        let response = client
            .post(self.endpoint.as_ref())
            .header(reqwest::header::USER_AGENT, DEFAULT_UA)
            .header("Client-Id", CLIENT_ID)
            .json(&body)
            .send()
            .await?;
        let response: UsersResponse = ExtractorError::check_response(response)?.json().await?;
        // Any error fails the whole answer: a field that failed to resolve comes back as `null`
        // beside the error, and a `null` stream would otherwise read as offline.
        if let Some(error) = response.errors.into_iter().next() {
            return Err(ExtractorError::ValidationError(format!(
                "Twitch batch status query failed: {}",
                error.message
            )));
        }
        let Some(users) = response.data.and_then(|data| data.users) else {
            return Err(ExtractorError::ValidationError(
                "Twitch batch status query returned no users".to_owned(),
            ));
        };

        // Matched by login rather than position, so the answer stays correct if Twitch drops or
        // reorders entries. Logins are case-insensitive and keys are already lowercase.
        let by_login: HashMap<String, BatchPresence> = users
            .into_iter()
            .flatten()
            .map(|user| {
                let presence = if user.stream.is_some() {
                    BatchPresence::Live
                } else {
                    BatchPresence::Offline
                };
                (user.login.to_ascii_lowercase(), presence)
            })
            .collect();
        Ok(keys
            .iter()
            .filter_map(|key| {
                let presence = by_login.get(key)?;
                Some((key.clone(), *presence))
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extractor::default::default_client;
    use crate::extractor::test_support::serve_once;

    async fn probe_against(
        body: &'static str,
        keys: &[&str],
    ) -> Result<HashMap<String, BatchPresence>, ExtractorError> {
        let probe = TwitchStatusProbe {
            endpoint: Cow::Owned(serve_once("200 OK", body).await),
        };
        let keys: Vec<String> = keys.iter().map(|key| (*key).to_owned()).collect();
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            probe.probe(&default_client(), &keys),
        )
        .await
        .expect("local GQL request timed out")
    }

    /// Entries are matched by login, so a reordered, shortened or differently cased answer still
    /// reaches the right channels, and a `null` user is left unknown.
    #[tokio::test]
    async fn users_are_matched_by_login_not_position() {
        let body = r#"{"data":{"users":[{"login":"Live_One","stream":{"id":"1"}},null,{"login":"offline_one","stream":null}]}}"#;

        let result = probe_against(body, &["offline_one", "missing", "live_one", "gone"])
            .await
            .unwrap();

        assert_eq!(
            result,
            HashMap::from([
                ("live_one".to_owned(), BatchPresence::Live),
                ("offline_one".to_owned(), BatchPresence::Offline),
            ])
        );
    }

    /// A rejected query and a field that failed to resolve are both errors; the failed field's
    /// `null` stream must not read as offline.
    #[tokio::test]
    async fn graphql_errors_are_an_error() {
        for (body, message) in [
            (
                r#"{"errors":[{"message":"Cannot query field \"typ\" on type \"Stream\"."}]}"#,
                "Cannot query field",
            ),
            (
                r#"{"data":{"users":[{"login":"foo","stream":null}]},"errors":[{"message":"service timeout","path":["users",0,"stream"]}]}"#,
                "service timeout",
            ),
        ] {
            let error = probe_against(body, &["foo"]).await.unwrap_err();

            assert!(
                matches!(&error, ExtractorError::ValidationError(text) if text.contains(message)),
                "{error:?}"
            );
        }
    }
}
