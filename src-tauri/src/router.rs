//! Choosing where an answer comes from by meaning, before the model is asked.
//!
//! **Experimental, and off unless `CHIEF_ROUTER_URL` names an embedding
//! server.** A small embedding model reads the question and a one-line
//! description of each source, and the closest description wins. Chief then
//! fetches that source itself and the main model only writes up what came
//! back, with no tools on offer — so the step small models are weakest at,
//! choosing a tool and writing its call, is not asked of them.
//!
//! It is a router and not a classifier you have to trust, which is what the
//! margin is for. A question is routed only when the best source clearly beats
//! the runner-up; anything closer, and every question whose best match is
//! "none of these", takes the tool loop exactly as it did before this module
//! existed. A miss costs what a question cost already. **A wrong route is the
//! failure to design against**, because it answers from the wrong data without
//! saying so, which is the reason the margin is a constant with a test that
//! fails when it is loosened.
//!
//! No keyword list is involved. The descriptions are written once per source,
//! the way an integration would describe itself, and a new source adds a line.

use crate::llama::{Client, Error};

/// The environment variable that points Chief at an embedding server on
/// loopback. Unset, the router does nothing.
pub const ROUTER_URL_VAR: &str = "CHIEF_ROUTER_URL";

/// How far the best source must beat the runner-up to be believed.
///
/// Measured, not chosen: on two sets of questions (40 and 36) with
/// EmbeddingGemma 2, 0.02 sent no question to the wrong source, where 0.0 sent
/// two. Cosine scores are not probabilities, so this is a gap between two
/// scores and not a confidence.
pub const MARGIN: f32 = 0.02;

/// The same gap between the best GitHub view and the runner-up, and larger
/// because the three views are closer to each other than the sources are.
///
/// Measured on 24 questions (0.02 sent 3 to the wrong view, 0.04 none, 13
/// routed) and then on 18 held-out ones (0.03 sent 1 wrong, 0.04 none, 4
/// routed). 0.04 was chosen after the first set, and the second only confirms
/// it did no harm; it also shows how few questions clear it, so most GitHub
/// questions take the tool loop until the view descriptions are better.
pub const VIEW_MARGIN: f32 = 0.04;

/// A place an answer can be read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    GithubPullRequests,
    Calendar,
    Mail,
}

/// Which pull requests a GitHub question is about. Reading all three and
/// leaving the model to pick put the wrong ones in front of it, so the view is
/// chosen the same way the source is, and a question that does not clearly
/// name one goes to the tool loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GithubView {
    Reviewing,
    Open,
    Merged,
}

/// What each view says about itself, in the order they are embedded.
const VIEWS: [(GithubView, &str); 3] = [
    (
        GithubView::Reviewing,
        "Pull requests other people asked me to review",
    ),
    (GithubView::Open, "My own pull requests that are still open"),
    (
        GithubView::Merged,
        "Pull requests of mine that have been merged",
    ),
];

/// Where a question is read from: the source, and for GitHub which pull
/// requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Github(GithubView),
    Calendar,
    Mail,
}

/// What each source says about itself, and "none of these", in the order
/// they are embedded. `None` is not a source: it is the answer that sends a
/// question to the tool loop.
const CHOICES: [(Option<Source>, &str); 4] = [
    (
        Some(Source::GithubPullRequests),
        "GitHub pull requests: mine, merged, or waiting on my review",
    ),
    (
        Some(Source::Calendar),
        "Outlook calendar: meetings, times, who is in them",
    ),
    (
        Some(Source::Mail),
        "Outlook inbox: recent email, who sent it, who is waiting on me",
    ),
    (
        None,
        "None of these: general chat, writing, or anything else",
    ),
];

/// A router pointed at the embedding server named by [`ROUTER_URL_VAR`], if
/// there is one and it is loopback.
pub fn configured() -> Option<Client> {
    let url = std::env::var(ROUTER_URL_VAR)
        .ok()
        .filter(|url| !url.is_empty())?;

    Client::with_base_url(&url).ok()
}

/// Cosine similarity. Zero for a vector with no length, so a broken embedding
/// can never look like a match.
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let length = |v: &[f32]| v.iter().map(|x| x * x).sum::<f32>().sqrt();
    let (la, lb) = (length(a), length(b));

    if la == 0.0 || lb == 0.0 {
        0.0
    } else {
        dot / (la * lb)
    }
}

/// The source the question is routed to, from its embedding and the four
/// choices' embeddings in [`CHOICES`] order. `None` means take the tool loop.
///
/// Pure, so the rule that decides whether a question is believed is tested
/// without a server.
fn pick(question: &[f32], choices: &[Vec<f32>], margin: f32) -> Option<Source> {
    if choices.len() != CHOICES.len() {
        return None;
    }

    let mut scored: Vec<(f32, usize)> = choices
        .iter()
        .enumerate()
        .map(|(index, choice)| (cosine(question, choice), index))
        .collect();

    scored.sort_by(|a, b| b.0.total_cmp(&a.0));

    let (best, runner_up) = (scored[0], scored[1]);

    if best.0 - runner_up.0 < margin {
        return None;
    }

    CHOICES[best.1].0
}

/// The view a GitHub question names, with the same rule as [`pick`]: the best
/// must clearly beat the runner-up or there is no answer.
fn pick_view(question: &[f32], views: &[Vec<f32>], margin: f32) -> Option<GithubView> {
    if views.len() != VIEWS.len() {
        return None;
    }

    let mut scored: Vec<(f32, usize)> = views
        .iter()
        .enumerate()
        .map(|(index, view)| (cosine(question, view), index))
        .collect();

    scored.sort_by(|a, b| b.0.total_cmp(&a.0));

    if scored[0].0 - scored[1].0 < margin {
        return None;
    }

    Some(VIEWS[scored[0].1].0)
}

/// The route for a question from its embedding and those of the four sources
/// and three GitHub views.
fn decide(
    question: &[f32],
    sources: &[Vec<f32>],
    views: &[Vec<f32>],
    margin: f32,
    view_margin: f32,
) -> Option<Route> {
    match pick(question, sources, margin)? {
        Source::GithubPullRequests => pick_view(question, views, view_margin).map(Route::Github),
        Source::Calendar => Some(Route::Calendar),
        Source::Mail => Some(Route::Mail),
    }
}

/// Where this question should be read from, or `None` to use the tool loop.
///
/// One request: the question, the four descriptions and the three GitHub views are embedded together.
/// An unreachable or confused embedding server is an error for the caller to
/// swallow, never a reason to fail the question.
pub async fn route(router: &Client, question: &str) -> Result<Option<Route>, Error> {
    let mut inputs = vec![question.to_string()];
    inputs.extend(
        CHOICES
            .iter()
            .map(|(_, description)| description.to_string()),
    );
    inputs.extend(VIEWS.iter().map(|(_, description)| description.to_string()));

    let mut vectors = router.embed(&inputs).await?;
    let question = vectors.remove(0);
    let views = vectors.split_off(CHOICES.len());

    Ok(decide(&question, &vectors, &views, MARGIN, VIEW_MARGIN))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llama::test_support::serve;

    /// Four choice vectors pointing four different ways.
    fn choices() -> Vec<Vec<f32>> {
        vec![
            vec![1.0, 0.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0, 0.0],
            vec![0.0, 0.0, 1.0, 0.0],
            vec![0.0, 0.0, 0.0, 1.0],
        ]
    }

    #[test]
    fn routes_a_question_that_clearly_matches_one_source() {
        assert_eq!(
            pick(&[0.1, 0.9, 0.1, 0.1], &choices(), MARGIN),
            Some(Source::Calendar)
        );
        assert_eq!(
            pick(&[0.9, 0.1, 0.1, 0.1], &choices(), MARGIN),
            Some(Source::GithubPullRequests)
        );
        assert_eq!(
            pick(&[0.1, 0.1, 0.9, 0.1], &choices(), MARGIN),
            Some(Source::Mail)
        );
    }

    #[test]
    fn a_question_closest_to_none_of_these_takes_the_tool_loop() {
        assert_eq!(pick(&[0.1, 0.1, 0.1, 0.9], &choices(), MARGIN), None);
    }

    /// The guard this module exists to keep. Two sources about equally close is
    /// a question this cannot place, and guessing is the one failure that gives
    /// an answer from the wrong data without saying so.
    #[test]
    fn two_sources_about_equally_close_is_not_routed() {
        let question = [0.70, 0.69, 0.1, 0.1];

        assert_eq!(pick(&question, &choices(), MARGIN), None);
        assert_eq!(
            pick(&question, &choices(), 0.0),
            Some(Source::GithubPullRequests),
            "without the margin the same question is routed, which is the point"
        );
    }

    #[test]
    fn a_missing_or_empty_embedding_is_never_a_match() {
        assert_eq!(pick(&[0.0, 0.0, 0.0, 0.0], &choices(), MARGIN), None);
        assert_eq!(pick(&[1.0, 0.0, 0.0, 0.0], &choices()[..3], MARGIN), None);
    }

    #[test]
    fn the_margin_is_the_measured_one() {
        // Loosening this sends more questions down a route nobody has checked.
        assert!((MARGIN - 0.02).abs() < f32::EPSILON);
    }

    /// The embedding server's reply for the question and the four choices.
    fn reply(vectors: &[[f32; 4]]) -> String {
        let data: Vec<_> = vectors
            .iter()
            .enumerate()
            .map(|(index, embedding)| serde_json::json!({ "index": index, "embedding": embedding }))
            .collect();

        serde_json::json!({ "data": data }).to_string()
    }

    fn views() -> Vec<Vec<f32>> {
        vec![
            vec![1.0, 0.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0, 0.0],
            vec![0.0, 0.0, 1.0, 0.0],
        ]
    }

    #[test]
    fn the_view_margin_is_the_measured_one() {
        // At 0.02 three of 24 questions went to the wrong view.
        assert!((VIEW_MARGIN - 0.04).abs() < f32::EPSILON);
    }

    #[test]
    fn a_github_question_names_one_view_or_goes_to_the_tool_loop() {
        let sources = choices();

        assert_eq!(
            decide(
                &[0.9, 0.1, 0.0, 0.0],
                &sources,
                &views(),
                MARGIN,
                VIEW_MARGIN
            ),
            Some(Route::Github(GithubView::Reviewing))
        );

        let close = vec![
            vec![1.0, 0.0, 0.0, 0.0],
            vec![0.99, 0.1, 0.0, 0.0],
            vec![0.0, 0.0, 1.0, 0.0],
        ];
        assert_eq!(
            decide(
                &[1.0, 0.05, 0.0, 0.0],
                &sources,
                &close,
                MARGIN,
                VIEW_MARGIN
            ),
            None,
            "two views about equally close must not pick one"
        );
        assert_eq!(
            decide(&[1.0, 0.05, 0.0, 0.0], &sources, &close, MARGIN, 0.0),
            Some(Route::Github(GithubView::Reviewing)),
            "without the margin the same question is routed"
        );
    }

    #[tokio::test]
    async fn embeds_the_question_and_every_description_in_one_request() {
        let (base_url, server) = serve(vec![(
            "HTTP/1.1 200 OK",
            reply(&[
                [0.1, 0.9, 0.1, 0.1],
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
            ]),
        )]);
        let router = Client::with_base_url(&base_url).expect("loopback should be allowed");

        let source = route(&router, "How busy is Friday?")
            .await
            .expect("the stub should answer");

        assert_eq!(source, Some(Route::Calendar));

        let requests = server.await.expect("the stub should finish");
        assert_eq!(requests.len(), 1);

        let body: serde_json::Value = serde_json::from_str(
            requests[0]
                .split_once("\r\n\r\n")
                .expect("request should have a body")
                .1,
        )
        .expect("body should be JSON");
        let inputs = body["input"].as_array().expect("inputs should be a list");

        assert_eq!(inputs.len(), 8);
        assert_eq!(inputs[0], "How busy is Friday?");
        assert!(inputs[2]
            .as_str()
            .is_some_and(|text| text.contains("calendar")));
    }

    #[tokio::test]
    async fn an_embedding_server_that_answers_wrongly_is_an_error_not_a_route() {
        let (base_url, _server) = serve(vec![("HTTP/1.1 200 OK", reply(&[[1.0, 0.0, 0.0, 0.0]]))]);
        let router = Client::with_base_url(&base_url).expect("loopback should be allowed");

        assert!(route(&router, "What is waiting on me?").await.is_err());
    }

    #[test]
    fn will_not_embed_your_questions_on_another_machine() {
        assert!(Client::with_base_url("http://example.com:11436").is_err());
    }
}
