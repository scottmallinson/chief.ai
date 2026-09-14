//! Jira and Confluence with an API token the user pasted.
//!
//! The other way in. [`super`] registers Chief with Atlassian's Remote MCP
//! server and needs nothing from the user but a browser click — where the
//! organisation allows it. Rovo is a paid add-on and an administrator can
//! switch it off, and when they have, the MCP server answers a connect attempt
//! with a refusal no amount of retrying fixes. This path needs no Rovo, no
//! registration and no administrator: a user makes an API token in their own
//! Atlassian account and pastes it.
//!
//! ## What the official documentation settles
//!
//! Both products take the same credential and differ only in path, which is
//! the whole reason one client serves both:
//!
//! - **Basic auth, not a bearer token.** `Authorization: Basic` over
//!   base64(`email:token`) — the email is half the credential, which is why
//!   this path asks for one and the MCP path does not.
//! - **The user's own site**, `https://<org>.atlassian.net`, and not
//!   `api.atlassian.com`. Those are different products: `api.atlassian.com/ex/jira/{cloudId}`
//!   is for OAuth and for Atlassian's newer *scoped* tokens, and mixing the
//!   two is a documented source of 401s. A classic API token goes to the site.
//! - **Jira search moved.** `GET`/`POST /rest/api/3/search` was removed and
//!   answers `410 Gone`; [`SEARCH`] is the replacement, which takes its
//!   arguments in a POST body, paginates by `nextPageToken` and no longer
//!   returns a total. Chief asks for one page and never pages, so only the
//!   first of those matters here.
//! - **Confluence lives under `/wiki`**, and its CQL search still supports
//!   `contributor = currentUser()` — the fields that were withdrawn from that
//!   endpoint are the user-search ones (`user`, `user.accountid`), not the
//!   content ones.
//!
//! ## Atlassian Cloud only, deliberately
//!
//! Everything above is Cloud's contract. **Jira and Confluence Data Center
//! and Server are a different one** and are not supported: their credential
//! is a *Personal Access Token*, which is a different object from a Cloud API
//! token, and it authenticates as `Authorization: Bearer <token>` with no
//! email at all. The two names get used interchangeably in conversation and
//! are not interchangeable on the wire.
//!
//! Supporting it would be small — an empty email meaning `Bearer` — and it is
//! left out because **nobody can test it here**. Cloud is what this project
//! can reach, and a second auth scheme that has never met a real server is a
//! branch that looks supported and is not. Recorded in §9 of the
//! implementation plan rather than built on a guess.
//!
//! A Data Center host is therefore *accepted* by [`site`] and fails at the
//! first request with [`Error::Rejected`]. That is not a mistake to fix by
//! narrowing [`site`] to `.atlassian.net`: **a Cloud site can be on a custom
//! domain**, so the host name does not tell the two apart, and refusing
//! custom domains would lock out Cloud customers to catch a case Chief does
//! not claim to serve.
//!
//! ## The token is a credential and the site is a decision
//!
//! The token is stored beside the OAuth tokens, never logged, never put in an
//! error and never returned to the frontend — the same rule [`crate::linear`]
//! follows, and a test asserts it.
//!
//! The **site address is the second thing in this app a person types that
//! decides where a credential goes** (the first is a calendar subscription),
//! so it takes the care `calendar.rs` takes: HTTPS only, no proxy, and every
//! redirect required to stay on HTTPS. [`site`] additionally completes a bare
//! name to `.atlassian.net`, because "acme" is what somebody has in their head
//! and "acme.atlassian.net" is where their Jira is — a typo that reaches a
//! stranger's server hands them a working token, and the narrower the input
//! the fewer typos survive.

use base64::Engine as _;
use serde::Deserialize;

use super::{because, host_of, Error, Issue, READ_LIMIT, TIMEOUT, USER_AGENT};

/// Where Jira's search moved to. The endpoint this replaced now answers 410.
const SEARCH: &str = "/rest/api/3/search/jql";

/// Who the token belongs to. Proves the credential before anything is stored,
/// the same shape as `linear::assigned` — one read that both validates and
/// names.
const MYSELF: &str = "/rest/api/3/myself";

/// Confluence's CQL search, which is under `/wiki` like everything else of
/// its.
const SEARCH_PAGES: &str = "/wiki/rest/api/search";

/// The suffix an Atlassian Cloud site has unless somebody says otherwise.
const CLOUD: &str = ".atlassian.net";

/// What is assigned to this person and not finished.
///
/// The same JQL the MCP path sends, and for the same reasons: built from
/// constants, never from anything the user typed, and `resolution = EMPTY`
/// rather than a workflow state called "Done", because every site renames
/// those and none of them can rename this.
const ASSIGNED: &str = "assignee = currentUser() AND resolution = EMPTY ORDER BY updated DESC";

/// Pages this person wrote or edited, most recently touched first.
///
/// `contributor` rather than `creator`: a page somebody else opened and this
/// person has been writing is theirs to be reminded of too.
const CONTRIBUTED: &str = "contributor = currentUser() and type = page order by lastmodified desc";

/// One Confluence page, reduced to what a brief says about it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Page {
    pub title: String,
    /// The space it lives in, when the search said.
    pub space: String,
    /// Where to open it.
    pub url: Option<String>,
}

/// Who a token belongs to, so an account can name itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Whoami {
    pub account_id: String,
    pub display_name: String,
}

/// Normalise what somebody typed into the address of their Atlassian site.
///
/// Accepts the four things people actually have to hand — a bare name, a
/// hostname, a full URL, and a URL with the path they were looking at still on
/// the end — and returns `https://host` with nothing after it.
///
/// **`http://` is refused rather than upgraded.** A calendar subscription
/// rewrites `webcal://` because that is the scheme providers hand out; nobody
/// hands out an `http://` Jira, so a person who typed one has either made a
/// mistake or is being redirected, and quietly making it HTTPS would hide
/// which.
pub fn site(typed: &str) -> Result<String, Error> {
    let typed = typed.trim();

    if typed.is_empty() {
        return Err(Error::Site(
            "an Atlassian site address is needed".to_string(),
        ));
    }

    if typed.starts_with("http://") {
        return Err(Error::Site(
            "an Atlassian site is reached over https, not http".to_string(),
        ));
    }

    // The scheme comes off before any trimming. Doing it the other way round
    // turned a bare "https://" into the host "https:", which then had no dot
    // and was completed to "https:.atlassian.net" — a test caught it.
    let rest = typed.strip_prefix("https://").unwrap_or(typed);
    let host = rest.split('/').next().unwrap_or_default().trim();

    // A port or userinfo means this is not the shape of address Atlassian
    // Cloud hands out, and both are ways to make a URL read as one host while
    // reaching another.
    if host.is_empty() || host.contains('@') || host.contains(':') || host.contains(' ') {
        return Err(Error::Site(format!("'{typed}' is not a site address")));
    }

    // A bare name is the common case and the one a typo survives least well.
    let host = if host.contains('.') {
        host.to_string()
    } else {
        format!("{host}{CLOUD}")
    };

    Ok(format!("https://{host}"))
}

/// Reads Jira and Confluence with a token the user pasted.
#[derive(Debug, Clone)]
pub struct Rest {
    http: reqwest::Client,
}

impl Rest {
    pub fn new() -> Result<Self, Error> {
        let http = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .user_agent(USER_AGENT)
            // The same care `calendar.rs` takes with the other address a
            // person types: no proxy, and a redirect may not leave HTTPS.
            .no_proxy()
            .redirect(https_only())
            .build()
            .map_err(|error| Error::Unreachable {
                host: "atlassian".to_string(),
                because: error.to_string(),
            })?;

        Ok(Self { http })
    }

    /// Who this token belongs to.
    ///
    /// Asked before anything is stored, so a mistyped token or site fails
    /// while the user is looking at the field rather than producing an empty
    /// brief tomorrow morning.
    pub async fn whoami(&self, site: &str, email: &str, token: &str) -> Result<Whoami, Error> {
        let found: MyselfReply = self.get(site, MYSELF, email, token, &[]).await?;

        Ok(Whoami {
            account_id: found.account_id,
            display_name: found.display_name,
        })
    }

    /// What is assigned to this person and not finished.
    pub async fn assigned(
        &self,
        site: &str,
        email: &str,
        token: &str,
    ) -> Result<Vec<Issue>, Error> {
        let body = serde_json::json!({
            "jql": ASSIGNED,
            "maxResults": READ_LIMIT,
            "fields": ["summary", "status"],
        });

        let found: SearchReply = self.post(site, SEARCH, email, token, &body).await?;

        Ok(found
            .issues
            .into_iter()
            .map(|issue| Issue {
                url: Some(format!("{site}/browse/{}", issue.key)),
                key: issue.key,
                summary: issue.fields.summary,
                status: issue.fields.status.map(|it| it.name).unwrap_or_default(),
            })
            .collect())
    }

    /// Pages this person has been writing.
    pub async fn pages(&self, site: &str, email: &str, token: &str) -> Result<Vec<Page>, Error> {
        let found: PagesReply = self
            .get(
                site,
                SEARCH_PAGES,
                email,
                token,
                &[("cql", CONTRIBUTED), ("limit", &READ_LIMIT.to_string())],
            )
            .await?;

        Ok(found
            .results
            .into_iter()
            .map(|hit| Page {
                title: if hit.title.is_empty() {
                    hit.content.title.clone()
                } else {
                    hit.title
                },
                space: hit.result_global_container.title,
                // Confluence returns a path, not a URL, and it is relative to
                // `/wiki` rather than to the site.
                url: hit
                    .url
                    .filter(|url| !url.is_empty())
                    .map(|url| format!("{site}/wiki{url}")),
            })
            .collect())
    }

    async fn get<T: serde::de::DeserializeOwned>(
        &self,
        site: &str,
        path: &str,
        email: &str,
        token: &str,
        query: &[(&str, &str)],
    ) -> Result<T, Error> {
        let url = format!("{site}{path}");

        let response = self
            .http
            .get(&url)
            .header("Authorization", basic(email, token))
            .header("Accept", "application/json")
            .query(query)
            .send()
            .await
            .map_err(|error| Error::from_transport(host_of(&url), because(&error)))?;

        self.read(response, &url).await
    }

    async fn post<T: serde::de::DeserializeOwned>(
        &self,
        site: &str,
        path: &str,
        email: &str,
        token: &str,
        body: &serde_json::Value,
    ) -> Result<T, Error> {
        let url = format!("{site}{path}");

        let response = self
            .http
            .post(&url)
            .header("Authorization", basic(email, token))
            .header("Accept", "application/json")
            .json(body)
            .send()
            .await
            .map_err(|error| Error::from_transport(host_of(&url), because(&error)))?;

        self.read(response, &url).await
    }

    async fn read<T: serde::de::DeserializeOwned>(
        &self,
        response: reqwest::Response,
        url: &str,
    ) -> Result<T, Error> {
        let status = response.status();

        // 401 is a token that is wrong or withdrawn; 403 is a token that is
        // right and not allowed to read this. Both are the user's to fix and
        // neither is worth retrying, which is what `Rejected` means to
        // `session.rs`.
        if status == 401 || status == 403 {
            return Err(Error::Rejected);
        }

        if !status.is_success() {
            return Err(Error::answered(url, status));
        }

        response.json().await.map_err(|_| Error::Decode)
    }
}

/// `Basic base64(email:token)`, as both products document.
fn basic(email: &str, token: &str) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(format!("{email}:{token}"));

    format!("Basic {encoded}")
}

/// A redirect policy that refuses to leave HTTPS.
///
/// Lifted in spirit from `calendar.rs`: a token is attached to every one of
/// these requests, and a redirect to `http://` would put it on the wire in
/// clear.
fn https_only() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.url().scheme() == "https" {
            attempt.follow()
        } else {
            attempt.stop()
        }
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MyselfReply {
    #[serde(default)]
    account_id: String,
    #[serde(default)]
    display_name: String,
}

#[derive(Debug, Deserialize)]
struct SearchReply {
    #[serde(default)]
    issues: Vec<IssueNode>,
}

#[derive(Debug, Deserialize)]
struct IssueNode {
    #[serde(default)]
    key: String,
    #[serde(default)]
    fields: Fields,
}

#[derive(Debug, Default, Deserialize)]
struct Fields {
    #[serde(default)]
    summary: String,
    #[serde(default)]
    status: Option<Named>,
}

#[derive(Debug, Deserialize)]
struct Named {
    #[serde(default)]
    name: String,
}

#[derive(Debug, Deserialize)]
struct PagesReply {
    #[serde(default)]
    results: Vec<PageHit>,
}

#[derive(Debug, Deserialize)]
struct PageHit {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    content: Titled,
    #[serde(default, rename = "resultGlobalContainer")]
    result_global_container: Titled,
}

#[derive(Debug, Default, Deserialize)]
struct Titled {
    #[serde(default)]
    title: String,
}

/// One page, as a line in the brief. The same shape as [`super::describe`].
#[must_use]
pub fn describe(page: &Page) -> String {
    if page.space.is_empty() {
        page.title.clone()
    } else {
        format!("{} [{}]", page.title, page.space)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llama::test_support::serve;

    // ---------------------------------------------------------------------
    // The site address, which is where a typo sends a credential.
    // ---------------------------------------------------------------------

    #[test]
    fn takes_the_four_things_people_actually_type() {
        for typed in [
            "acme",
            "acme.atlassian.net",
            "https://acme.atlassian.net",
            "https://acme.atlassian.net/",
            "https://acme.atlassian.net/jira/software/projects/ENG/boards/1",
        ] {
            assert_eq!(
                site(typed).expect("should normalise"),
                "https://acme.atlassian.net",
                "from {typed}"
            );
        }
    }

    /// A site on a custom domain is still a site — completion is for the bare
    /// name, not a rule about where Jira may live.
    ///
    /// **This is why `site` cannot be narrowed to `.atlassian.net` to keep
    /// Data Center out.** A Cloud site can be on a custom domain, so the host
    /// does not say which product is behind it; a Data Center host is
    /// accepted here and refused at the first request instead. See the module
    /// docs.
    #[test]
    fn leaves_a_host_that_already_has_a_domain_alone() {
        assert_eq!(
            site("jira.acme.co.uk").expect("should normalise"),
            "https://jira.acme.co.uk"
        );
    }

    /// **`http://` is refused rather than upgraded.** A calendar rewrites
    /// `webcal://` because that is the scheme providers hand out; nobody hands
    /// out an `http://` Jira, so silently making it HTTPS would hide a mistake
    /// rather than fix one.
    ///
    /// **The message is asserted, not just the refusal.** Deleting the scheme
    /// check leaves this address refused anyway — `http:` keeps its colon and
    /// trips the port guard below — so `is_err()` alone cannot tell the branch
    /// is there. What it costs is the only thing that helps: somebody told
    /// their address is "not a site address" goes looking for a typo in the
    /// host.
    #[test]
    fn says_an_http_address_is_the_wrong_scheme_rather_than_a_bad_host() {
        let error = site("http://acme.atlassian.net").expect_err("should be refused");

        assert!(
            error.to_string().contains("https"),
            "the scheme is what is wrong, and saying so is the point: {error}"
        );
    }

    #[test]
    fn refuses_anything_that_is_not_a_site_over_https() {
        for hostile in [
            "",
            "   ",
            "https://",
            "https://user@evil.test",
            "acme atlassian net",
            // Completed to "https:.atlassian.net" before the scheme was
            // stripped ahead of the trim.
            "https:/",
            "acme.atlassian.net:8443",
        ] {
            assert!(site(hostile).is_err(), "{hostile} should be refused");
        }
    }

    // ---------------------------------------------------------------------
    // Reading.
    // ---------------------------------------------------------------------

    fn issues_body() -> String {
        serde_json::json!({
            "issues": [
                { "key": "ENG-7", "fields": {
                    "summary": "Migrate the tenant", "status": { "name": "In Review" } } },
                { "key": "ENG-9", "fields": { "summary": "Rotate the keys" } },
            ]
        })
        .to_string()
    }

    #[tokio::test]
    async fn reads_what_is_assigned_and_unfinished() {
        let (host, server) = serve(vec![("HTTP/1.1 200 OK", issues_body())]);
        let rest = Rest::new().expect("client");

        let found = rest
            .assigned(&host, "scott@example.com", "a-token")
            .await
            .expect("should read");

        assert_eq!(found.len(), 2);
        assert_eq!(found[0].key, "ENG-7");
        assert_eq!(found[0].status, "In Review");
        assert_eq!(
            found[0].url.as_deref(),
            Some(&*format!("{host}/browse/ENG-7"))
        );
        assert!(
            found[1].status.is_empty(),
            "an issue with no status is still an issue"
        );

        let sent = server.await.expect("server").join("");

        assert!(
            sent.contains("/rest/api/3/search/jql"),
            "the endpoint this replaced answers 410 Gone: {sent}"
        );
        assert!(sent.contains("currentUser()"), "{sent}");
        assert!(
            sent.contains("resolution = EMPTY"),
            "open is the absence of a resolution, not a state called Done: {sent}"
        );
    }

    fn pages_body() -> String {
        serde_json::json!({
            "results": [{
                "title": "Tenant migration runbook",
                "url": "/spaces/ENG/pages/123",
                "content": { "title": "Tenant migration runbook" },
                "resultGlobalContainer": { "title": "Engineering" },
            }]
        })
        .to_string()
    }

    #[tokio::test]
    async fn reads_the_pages_this_person_has_been_writing() {
        let (host, server) = serve(vec![("HTTP/1.1 200 OK", pages_body())]);
        let rest = Rest::new().expect("client");

        let found = rest
            .pages(&host, "scott@example.com", "a-token")
            .await
            .expect("should read");

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].title, "Tenant migration runbook");
        assert_eq!(found[0].space, "Engineering");
        assert_eq!(
            found[0].url.as_deref(),
            Some(&*format!("{host}/wiki/spaces/ENG/pages/123")),
            "Confluence returns a path relative to /wiki, not a URL"
        );

        let sent = server.await.expect("server").join("");

        assert!(sent.contains("/wiki/rest/api/search"), "{sent}");
        assert!(
            sent.contains("contributor") && sent.contains("currentUser"),
            "a page somebody else opened and this person edited is theirs too: {sent}"
        );
    }

    /// Basic auth, not a bearer token — the email is half the credential,
    /// which is the whole reason this path asks for one.
    #[tokio::test]
    async fn sends_the_email_and_token_as_http_basic() {
        let (host, server) = serve(vec![("HTTP/1.1 200 OK", issues_body())]);
        let rest = Rest::new().expect("client");

        rest.assigned(&host, "scott@example.com", "a-token")
            .await
            .expect("should read");

        let sent = server.await.expect("server").join("").to_lowercase();
        let expected = base64::engine::general_purpose::STANDARD
            .encode("scott@example.com:a-token")
            .to_lowercase();

        assert!(
            sent.contains(&format!("authorization: basic {expected}")),
            "{sent}"
        );
    }

    /// **The credential must never be on the wire in clear.** A token rides on
    /// every one of these requests, so a query string is the wrong place for
    /// any part of it. A URL is logged by every proxy it passes, kept in
    /// browser history and repeated back in a `Referer`; a header is not.
    ///
    /// Both verbs, because they fail differently. `pages` already builds a
    /// query string, so one more pair there looks like the others; `assigned`
    /// has none, so adding one is conspicuous.
    ///
    /// **Asserted against pieces percent-encoding cannot touch.** The first
    /// version of this test looked for `scott@example.com` and passed with
    /// the address in the query string, because reqwest had written it as
    /// `scott%40example.com`. `scott`, `example.com` and the token itself
    /// survive any encoding of the address that still names the same person.
    #[tokio::test]
    async fn never_puts_the_credential_in_the_url() {
        for (verb, replies) in [
            ("POST", vec![("HTTP/1.1 200 OK", issues_body())]),
            ("GET", vec![("HTTP/1.1 200 OK", pages_body())]),
        ] {
            let (host, server) = serve(replies);
            let rest = Rest::new().expect("client");

            if verb == "POST" {
                rest.assigned(&host, "scott@example.com", "atl-secret")
                    .await
                    .expect("should read");
            } else {
                rest.pages(&host, "scott@example.com", "atl-secret")
                    .await
                    .expect("should read");
            }

            let sent = server.await.expect("server");

            assert!(
                !sent.is_empty(),
                "{verb}: nothing was sent, so this proves nothing"
            );

            let line = sent[0].lines().next().unwrap_or_default();

            for piece in ["atl-secret", "scott", "example.com"] {
                assert!(
                    !line.contains(piece),
                    "{verb}: the request line carries '{piece}', which is part of the credential: {line}"
                );
            }
        }
    }

    /// A rejected token and a token that is not allowed to read are both the
    /// user's to fix, and neither is worth retrying.
    #[tokio::test]
    async fn a_refused_token_says_so_rather_than_looking_like_an_outage() {
        for status in ["HTTP/1.1 401 Unauthorized", "HTTP/1.1 403 Forbidden"] {
            let (host, _server) = serve(vec![(status, "{}")]);
            let rest = Rest::new().expect("client");

            let error = rest
                .assigned(&host, "scott@example.com", "wrong")
                .await
                .expect_err("should refuse");

            assert!(matches!(error, Error::Rejected), "{status}: {error:?}");
        }
    }

    #[tokio::test]
    async fn no_error_carries_the_token() {
        let (host, _server) = serve(vec![("HTTP/1.1 500 Internal Server Error", "{}")]);
        let rest = Rest::new().expect("client");

        let error = rest
            .assigned(&host, "scott@example.com", "atl-token-do-not-print")
            .await
            .expect_err("should fail");

        assert!(
            !error.to_string().contains("atl-token-do-not-print"),
            "{error}"
        );
    }

    #[test]
    fn describes_a_page_as_one_line() {
        let page = Page {
            title: "Tenant migration runbook".to_string(),
            space: "Engineering".to_string(),
            url: None,
        };

        assert_eq!(describe(&page), "Tenant migration runbook [Engineering]");

        let homeless = Page {
            space: String::new(),
            ..page
        };

        assert_eq!(describe(&homeless), "Tenant migration runbook");
    }
}
