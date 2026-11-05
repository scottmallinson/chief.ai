//! The trackers and calendars a user has connected, each read the same way.
//!
//! "What should I prioritise?" and "draft my standup" are about the user's
//! work, and which tools that work lives in is the user's choice: some have
//! GitHub, some Linear or Jira, some only a calendar. So nothing that answers
//! them may assume one. Each read here reports whether anything of its kind is
//! connected, whether anything answered, and what failed — three different
//! statements that an empty list would run together.
//!
//! The brief gathers these same sources in `recipe::gather`, which swallows
//! failures because a brief with a calendar and no mail is worth having. A
//! question has to say what it could not read, so it asks here.

use chrono::NaiveDate;

use crate::atlassian;
use crate::integrations;
use crate::linear;
use crate::microsoft;
use crate::recipe::{self, Reach, Route};
use crate::session::AtlassianSession;

/// What reading one kind of source found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found<T> {
    pub items: Vec<T>,
    /// An account of this kind is connected. Without one the source is simply
    /// not part of the user's setup and is not mentioned.
    pub connected: bool,
    /// At least one account answered, so an empty `items` means "nothing".
    pub answered: bool,
    /// Why each account that did not answer could not be read. Never carries a
    /// credential: every error type involved is written not to.
    pub failed: Vec<String>,
}

impl<T> Default for Found<T> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            connected: false,
            answered: false,
            failed: Vec::new(),
        }
    }
}

impl<T> Found<T> {
    pub(crate) fn ok(&mut self, mut items: Vec<T>) {
        self.answered = true;
        self.items.append(&mut items);
    }

    pub(crate) fn fail(&mut self, why: impl ToString) {
        self.failed.push(why.to_string());
    }
}

/// Linear issues: what is assigned and open, or, given a date, what the user
/// completed on or after it.
pub async fn linear(context: &impl Reach, since: Option<NaiveDate>) -> Found<linear::Issue> {
    let mut found = Found::default();

    let Ok(accounts) = integrations::accounts(context.pool(), integrations::LINEAR).await else {
        return found;
    };

    for account in accounts {
        found.connected = true;

        let Ok(Some(credentials)) = integrations::credentials(context.pool(), account.id).await
        else {
            found.fail("Linear has no key saved");
            continue;
        };

        let read = match since {
            None => context.linear().assigned(&credentials.access_token).await,
            Some(day) => {
                context
                    .linear()
                    .completed(
                        &credentials.access_token,
                        &day.format("%Y-%m-%d").to_string(),
                    )
                    .await
            }
        };

        match read {
            Ok(read) => found.ok(read.issues),
            Err(error) => found.fail(error),
        }
    }

    found
}

/// Jira issues, by whichever route each account was connected: what is
/// assigned and open, or, given a date, what was resolved on or after it.
pub async fn jira(context: &impl Reach, since: Option<NaiveDate>) -> Found<atlassian::Issue> {
    let mut found = Found::default();

    let Ok(accounts) = integrations::accounts(context.pool(), integrations::ATLASSIAN).await else {
        return found;
    };

    for account in accounts {
        found.connected = true;

        let Ok(Some(credentials)) = integrations::credentials(context.pool(), account.id).await
        else {
            found.fail("Jira has no credential saved");
            continue;
        };

        let read = match recipe::route(&account, &credentials) {
            Route::Mcp => {
                let session =
                    AtlassianSession::new(context.pool(), context.atlassian(), account.id);

                match since {
                    None => session.assigned().await,
                    Some(day) => session.completed(day).await,
                }
            }
            Route::Rest { site, email } => match since {
                None => {
                    context
                        .atlassian_rest()
                        .assigned(&site, &email, &credentials.access_token)
                        .await
                }
                Some(day) => {
                    context
                        .atlassian_rest()
                        .completed(&site, &email, &credentials.access_token, day)
                        .await
                }
            },
        };

        match read {
            Ok(issues) => found.ok(issues),
            Err(error) => found.fail(error),
        }
    }

    found
}

/// Today's events from the calendars subscribed to by address.
pub async fn subscribed(
    context: &impl Reach,
    now: chrono::DateTime<chrono::Local>,
) -> Found<microsoft::Event> {
    let mut found = Found::default();

    for (_, read) in recipe::read_subscriptions_at(context, now).await {
        found.connected = true;

        match read {
            Some(events) => found.ok(events),
            None => found.fail("a subscribed calendar could not be read"),
        }
    }

    found
}
