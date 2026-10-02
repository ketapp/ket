//! OpenCode Go's quota fetch — the keyed usage API, the console-cookie
//! fallback, and the refusals each path can report — all against fake
//! `HttpGet`/`OpenCodeCredentials`/`OpenCodeKeyStore` implementations rather
//! than the network.

use std::sync::Arc;
use std::time::Duration;

use ket_core::config::OpenCodeGoConfig;
use ket_core::rate_limits::{
    HttpGet, HttpRequest, OpenCodeCredentials, OpenCodeKeyStore, OpenCodeRateLimitSource, Provider,
    RateLimitSource, SnapshotStatus, normalize_cookie,
};

#[derive(Debug, Default)]
struct FakeCredentials(OpenCodeGoConfig);

impl OpenCodeCredentials for FakeCredentials {
    fn read(&self) -> OpenCodeGoConfig {
        self.0.clone()
    }
}

fn creds(api_key: &str, session_cookie: &str, workspace_id: &str) -> Arc<dyn OpenCodeCredentials> {
    Arc::new(FakeCredentials(OpenCodeGoConfig {
        api_key: api_key.to_owned(),
        session_cookie: session_cookie.to_owned(),
        workspace_id: workspace_id.to_owned(),
    }))
}

fn no_creds() -> Arc<dyn OpenCodeCredentials> {
    creds("", "", "")
}

#[derive(Debug)]
struct FakeKeyStore(Option<&'static str>);

impl OpenCodeKeyStore for FakeKeyStore {
    fn go_key(&self) -> Option<String> {
        self.0.map(str::to_owned)
    }
}

fn no_key() -> Arc<dyn OpenCodeKeyStore> {
    Arc::new(FakeKeyStore(None))
}

/// One canned answer per URL, matched by whether the URL contains a marker
/// substring — good enough to tell `/zen/go/v1/usage` from `/orgs` from
/// `/go/status`.
#[derive(Debug, Default)]
struct ScriptedHttp {
    routes: Vec<(&'static str, u16, &'static str)>,
}

impl ScriptedHttp {
    fn route(mut self, marker: &'static str, status: u16, body: &'static str) -> Self {
        self.routes.push((marker, status, body));
        self
    }
}

impl HttpGet for ScriptedHttp {
    fn get(&self, request: &HttpRequest, _timeout: Duration) -> ket_core::Result<(u16, String)> {
        for (marker, status, body) in &self.routes {
            if request.url.contains(marker) {
                return Ok((*status, (*body).to_owned()));
            }
        }
        panic!("unscripted request: {request:?}");
    }
}

fn timeout() -> Duration {
    Duration::from_secs(1)
}

/// The `reason` of an `Unavailable` snapshot, or a panic naming what it
/// actually was.
fn reason(snapshot: &ket_core::rate_limits::ProviderSnapshot) -> &str {
    match &snapshot.status {
        SnapshotStatus::Unavailable { reason } => reason,
        other => panic!("expected Unavailable, got {other:?}"),
    }
}

// ---- normalize_cookie / auth_cookies_only (via the fetch path) --------------

#[test]
fn normalize_cookie_wraps_a_bare_token_and_leaves_a_header_alone() {
    assert_eq!(
        normalize_cookie("st_abc123"),
        "__Host-console_session=st_abc123"
    );
    assert_eq!(
        normalize_cookie("__Host-console_session=st_abc123; other=1"),
        "__Host-console_session=st_abc123; other=1"
    );
}

#[test]
fn normalize_cookie_of_an_empty_string_is_empty() {
    assert_eq!(normalize_cookie(""), "");
    assert_eq!(normalize_cookie("   "), "");
}

// ---- the keyed path -----------------------------------------------------

#[test]
fn a_keyed_fetch_succeeds_on_a_valid_usage_response() {
    let body = r#"{"usage":{"rolling":{"percent":12.5,"resetsAt":"2026-01-01T00:00:00Z"},"weekly":{"percent":40.0}}}"#;
    let http = Arc::new(ScriptedHttp::default().route("/zen/go/v1/usage", 200, body));
    let source =
        OpenCodeRateLimitSource::with(http, no_creds(), Arc::new(FakeKeyStore(Some("key-1"))));

    let snapshot = source.fetch(timeout());

    assert_eq!(snapshot.status, SnapshotStatus::Fresh);
    assert_eq!(snapshot.provider, Provider::OpenCode);
    assert_eq!(snapshot.plan.as_deref(), Some("Go"));
    assert_eq!(snapshot.windows.len(), 2, "no monthly meter in the body");
    assert_eq!(snapshot.windows[0].used_percent, 12.5);
    assert!(snapshot.windows[0].resets_at_ms.is_some());
    assert_eq!(snapshot.windows[1].used_percent, 40.0);
}

#[test]
fn a_keyed_fetch_includes_the_monthly_window_when_the_body_has_one() {
    let body = r#"{"usage":{"rolling":{"percent":1.0},"weekly":{"percent":2.0},"monthly":{"percent":3.0}}}"#;
    let http = Arc::new(ScriptedHttp::default().route("/zen/go/v1/usage", 200, body));
    let source =
        OpenCodeRateLimitSource::with(http, no_creds(), Arc::new(FakeKeyStore(Some("key-1"))));

    let snapshot = source.fetch(timeout());
    assert_eq!(snapshot.windows.len(), 3);
    assert_eq!(snapshot.windows[2].used_percent, 3.0);
}

#[test]
fn a_percent_over_a_hundred_is_clamped() {
    let body = r#"{"usage":{"rolling":{"percent":150.0},"weekly":{"percent":-5.0}}}"#;
    let http = Arc::new(ScriptedHttp::default().route("/zen/go/v1/usage", 200, body));
    let source =
        OpenCodeRateLimitSource::with(http, no_creds(), Arc::new(FakeKeyStore(Some("key-1"))));

    let snapshot = source.fetch(timeout());
    assert_eq!(snapshot.windows[0].used_percent, 100.0);
    assert_eq!(snapshot.windows[1].used_percent, 0.0);
}

#[test]
fn an_unauthorized_key_is_reported_as_rejected() {
    let body = r#"{"error":{"type":"AuthError"}}"#;
    let http = Arc::new(ScriptedHttp::default().route("/zen/go/v1/usage", 401, body));
    let source =
        OpenCodeRateLimitSource::with(http, no_creds(), Arc::new(FakeKeyStore(Some("bad-key"))));

    let snapshot = source.fetch(timeout());
    assert!(matches!(
        snapshot.status,
        SnapshotStatus::Unavailable { .. }
    ));
    assert!(
        reason(&snapshot).contains("rejected"),
        "{:?}",
        snapshot.status
    );
}

#[test]
fn a_key_with_no_subscription_is_reported_by_name() {
    let body = r#"{"error":{"type":"EntitlementError"}}"#;
    let http = Arc::new(ScriptedHttp::default().route("/zen/go/v1/usage", 403, body));
    let source =
        OpenCodeRateLimitSource::with(http, no_creds(), Arc::new(FakeKeyStore(Some("key-1"))));

    let snapshot = source.fetch(timeout());
    assert!(matches!(
        snapshot.status,
        SnapshotStatus::Unavailable { .. }
    ));
    assert!(
        reason(&snapshot).contains("no OpenCode Go subscription"),
        "{:?}",
        snapshot.status
    );
}

#[test]
fn a_403_status_with_no_error_body_is_still_classified_as_no_subscription() {
    let http = Arc::new(ScriptedHttp::default().route("/zen/go/v1/usage", 403, ""));
    let source =
        OpenCodeRateLimitSource::with(http, no_creds(), Arc::new(FakeKeyStore(Some("key-1"))));
    let snapshot = source.fetch(timeout());
    assert!(reason(&snapshot).contains("no OpenCode Go subscription"));
}

#[test]
fn a_key_bounced_to_the_sign_in_page_is_treated_as_unauthorized() {
    let http =
        Arc::new(ScriptedHttp::default().route("/zen/go/v1/usage", 200, "<html>sign in</html>"));
    let source =
        OpenCodeRateLimitSource::with(http, no_creds(), Arc::new(FakeKeyStore(Some("key-1"))));
    let snapshot = source.fetch(timeout());
    assert!(reason(&snapshot).contains("rejected"));
}

#[test]
fn an_unparseable_usage_body_is_a_failure_naming_the_problem() {
    let http = Arc::new(ScriptedHttp::default().route("/zen/go/v1/usage", 200, "not json"));
    let source =
        OpenCodeRateLimitSource::with(http, no_creds(), Arc::new(FakeKeyStore(Some("key-1"))));
    let snapshot = source.fetch(timeout());
    assert!(matches!(
        snapshot.status,
        SnapshotStatus::Unavailable { .. }
    ));
    assert!(reason(&snapshot).contains("not understood"));
}

#[test]
fn an_other_status_is_reported_verbatim() {
    let http = Arc::new(ScriptedHttp::default().route("/zen/go/v1/usage", 500, ""));
    let source =
        OpenCodeRateLimitSource::with(http, no_creds(), Arc::new(FakeKeyStore(Some("key-1"))));
    let snapshot = source.fetch(timeout());
    assert!(reason(&snapshot).contains("500"));
}

// ---- cookie fallback and the console path --------------------------------

#[test]
fn a_keyed_refusal_falls_through_to_a_working_cookie() {
    let usage_body = r#"{"error":{"type":"EntitlementError"}}"#;
    let orgs_body = r#"[{"id":"org_abc123"}]"#;
    let status_body = r#"{"access":{"meters":{"fiveHour":{"limitMicroCents":0,"usedMicroCents":0},"week":{"limitMicroCents":0,"usedMicroCents":0}}}}"#
        .to_owned();
    let http = Arc::new(
        ScriptedHttp::default()
            .route("/zen/go/v1/usage", 403, usage_body)
            .route(
                "/orgs",
                200,
                Box::leak(orgs_body.to_owned().into_boxed_str()),
            )
            .route("/go/status", 200, Box::leak(status_body.into_boxed_str())),
    );
    let source = OpenCodeRateLimitSource::with(
        http,
        creds("key-1", "st_abc", ""),
        Arc::new(FakeKeyStore(None)),
    );

    let snapshot = source.fetch(timeout());
    assert_eq!(snapshot.status, SnapshotStatus::Fresh);
}

#[test]
fn no_key_and_no_cookie_is_unsupported_without_any_request() {
    #[derive(Debug)]
    struct NeverFetches;
    impl HttpGet for NeverFetches {
        fn get(
            &self,
            request: &HttpRequest,
            _timeout: Duration,
        ) -> ket_core::Result<(u16, String)> {
            panic!("should not fetch: {request:?}");
        }
    }

    let source = OpenCodeRateLimitSource::with(Arc::new(NeverFetches), no_creds(), no_key());
    let snapshot = source.fetch(timeout());
    assert_eq!(snapshot.status, SnapshotStatus::Unsupported);
    assert!(snapshot.windows.is_empty());
}

#[test]
fn a_cookie_that_carries_no_session_is_reported_rather_than_guessed_at() {
    #[derive(Debug)]
    struct NeverFetches;
    impl HttpGet for NeverFetches {
        fn get(
            &self,
            request: &HttpRequest,
            _timeout: Duration,
        ) -> ket_core::Result<(u16, String)> {
            panic!("should not fetch: {request:?}");
        }
    }

    let source = OpenCodeRateLimitSource::with(
        Arc::new(NeverFetches),
        creds("", "unrelated=1; other=2", ""),
        no_key(),
    );
    let snapshot = source.fetch(timeout());
    assert!(matches!(
        snapshot.status,
        SnapshotStatus::Unavailable { .. }
    ));
    assert!(
        reason(&snapshot).contains("no session cookie"),
        "{:?}",
        snapshot.status
    );
}

#[test]
fn a_console_session_rejected_at_401_says_to_sign_in_again() {
    let http = Arc::new(ScriptedHttp::default().route("/orgs", 401, ""));
    let source = OpenCodeRateLimitSource::with(http, creds("", "st_abc", ""), no_key());
    let snapshot = source.fetch(timeout());
    assert!(reason(&snapshot).contains("sign in again"));
}

#[test]
fn a_sign_in_page_returned_as_200_is_treated_as_a_lost_session() {
    let http = Arc::new(ScriptedHttp::default().route("/orgs", 200, "<html>sign in</html>"));
    let source = OpenCodeRateLimitSource::with(http, creds("", "st_abc", ""), no_key());
    let snapshot = source.fetch(timeout());
    assert!(reason(&snapshot).contains("does not recognise"));
}

#[test]
fn no_organisations_found_is_reported_plainly() {
    let http = Arc::new(ScriptedHttp::default().route("/orgs", 200, "[]"));
    let source = OpenCodeRateLimitSource::with(http, creds("", "st_abc", ""), no_key());
    let snapshot = source.fetch(timeout());
    assert!(reason(&snapshot).contains("no organisation found"));
}

#[test]
fn non_organisation_ids_from_orgs_are_filtered_out() {
    let http = Arc::new(ScriptedHttp::default().route("/orgs", 200, r#"[{"id":"not-an-org"}]"#));
    let source = OpenCodeRateLimitSource::with(http, creds("", "st_abc", ""), no_key());
    let snapshot = source.fetch(timeout());
    assert!(reason(&snapshot).contains("no organisation found"));
}

#[test]
fn an_invalid_workspace_override_is_refused_before_any_request() {
    #[derive(Debug)]
    struct NeverFetches;
    impl HttpGet for NeverFetches {
        fn get(
            &self,
            request: &HttpRequest,
            _timeout: Duration,
        ) -> ket_core::Result<(u16, String)> {
            panic!("should not fetch: {request:?}");
        }
    }

    let source = OpenCodeRateLimitSource::with(
        Arc::new(NeverFetches),
        creds("", "st_abc", "not-an-org-id"),
        no_key(),
    );
    let snapshot = source.fetch(timeout());
    assert!(
        reason(&snapshot).contains("is not an organisation id"),
        "{:?}",
        snapshot.status
    );
}

#[test]
fn a_valid_workspace_override_is_used_instead_of_discovery() {
    let status_body = r#"{"access":{"meters":{"fiveHour":{"limitMicroCents":0,"usedMicroCents":0},"week":{"limitMicroCents":0,"usedMicroCents":0}}}}"#;
    let http = Arc::new(ScriptedHttp::default().route("/go/status", 200, status_body));
    let source = OpenCodeRateLimitSource::with(http, creds("", "st_abc", "org_override"), no_key());
    let snapshot = source.fetch(timeout());
    assert_eq!(snapshot.status, SnapshotStatus::Fresh);
}

#[test]
fn a_subscription_with_no_access_yet_reports_why() {
    let orgs_body = r#"[{"id":"org_abc"}]"#;
    let status_body = r#"{"access":null}"#;
    let http = Arc::new(
        ScriptedHttp::default()
            .route("/orgs", 200, orgs_body)
            .route("/go/status", 200, status_body),
    );
    let source = OpenCodeRateLimitSource::with(http, creds("", "st_abc", ""), no_key());
    let snapshot = source.fetch(timeout());
    assert!(
        reason(&snapshot).contains("grants no access"),
        "{:?}",
        snapshot.status
    );
}

#[test]
fn an_organisation_with_no_subscription_is_skipped_for_the_next_one() {
    // /orgs lists two organisations; the first has no Go status (null body),
    // the second has a working one. Both share the /go/status marker, so the
    // scripted client can only answer one way — this exercises the org-id
    // header being threaded through by using a single matching org instead,
    // and confirms the "no organisation has Go" message when truly none do.
    let orgs_body = r#"[{"id":"org_one"},{"id":"org_two"}]"#;
    let http = Arc::new(
        ScriptedHttp::default()
            .route("/orgs", 200, orgs_body)
            .route("/go/status", 200, "null"),
    );
    let source = OpenCodeRateLimitSource::with(http, creds("", "st_abc", ""), no_key());
    let snapshot = source.fetch(timeout());
    assert!(
        reason(&snapshot).contains("no organisation on this account has OpenCode Go"),
        "{:?}",
        snapshot.status
    );
}

#[test]
fn an_oversized_response_is_refused_without_parsing_it() {
    let huge: String = "x".repeat(1024 * 1024 + 1);
    let http = Arc::new(ScriptedHttp::default().route(
        "/zen/go/v1/usage",
        200,
        Box::leak(huge.into_boxed_str()),
    ));
    let source =
        OpenCodeRateLimitSource::with(http, no_creds(), Arc::new(FakeKeyStore(Some("key-1"))));
    let snapshot = source.fetch(timeout());
    assert!(matches!(
        snapshot.status,
        SnapshotStatus::Unavailable { .. }
    ));
}

#[test]
fn provider_is_opencode() {
    let source =
        OpenCodeRateLimitSource::with(Arc::new(ScriptedHttp::default()), no_creds(), no_key());
    assert_eq!(source.provider(), Provider::OpenCode);
}
