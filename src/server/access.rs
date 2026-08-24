//! Who is allowed to talk to the HTTP servers (SRV-007).
//!
//! Both `nao watch` and `nao serve` bind loopback, which is not a security
//! boundary against a *browser*: the browser is on the same machine, so any
//! page the user has open can issue `fetch('http://localhost:3000/api/details')`
//! and read back entity source code. The only thing standing between a
//! visited page and the analyzed repo is the CORS policy, and until this
//! module existed that policy was `CorsLayer::permissive()` — every origin,
//! every method, every header.
//!
//! So the allowlist is deliberately small: the VS Code webview (which the
//! extension needs and which the user cannot type), plus whatever the user
//! named on the command line. Everything else gets no CORS headers and the
//! browser refuses to hand the response to the page.
//!
//! Non-browser clients (curl, an agent) are unaffected by the allowlist by
//! design. CORS is enforced by the browser, not the server.
//!
//! # The pairing token (SRV-009)
//!
//! An allowlist entry trusts a whole origin, forever, including whatever an
//! XSS on that site can run. That is an acceptable trade for
//! `http://localhost:4173` — a UI the user started themselves — and not for
//! `https://some.host`. So origins that are neither loopback nor the webview
//! have to present a per-process token as well, which the user copies out of
//! the startup banner.
//!
//! It is deliberately not authentication: no accounts, no sessions, nothing
//! persisted. It exists so that reaching a local engine from a remote page is
//! an act the *user* performed rather than one the page performed for them.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use axum::{
    extract::{Request, State},
    http::{header, HeaderValue, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use tower_http::cors::{AllowOrigin, CorsLayer};

/// The `vscode-webview://<guid>` scheme the extension's webview runs on.
/// Always allowed: the extension is the primary surface, the origin is
/// generated per window, and no user could allowlist it by hand.
const WEBVIEW_SCHEME: &str = "vscode-webview://";

/// What the CLI knows about who may reach this server.
///
/// Shared by `watch` and `serve` so a flag added here reaches both.
#[derive(Clone, Debug, Default)]
pub struct AccessOptions {
    /// Raw `--allow-origin` values, in the order given.
    pub allow_origin: Vec<String>,
    /// `--no-token`: mint no pairing token, so an allowlisted remote origin
    /// needs nothing beyond being allowlisted. The door is the user's to open.
    pub no_token: bool,
}

/// Query parameter carrying the pairing token, for clients that cannot set
/// headers. `EventSource` is the one that matters (UI-033): the SSE stream
/// has no way to send `Authorization` at all.
pub const TOKEN_QUERY_PARAM: &str = "token";

/// How many distinct refused origins get a log line before the server stops
/// reporting them. A refusal is a diagnostic, and a page can manufacture
/// origins faster than anyone will read them.
const MAX_REPORTED_REFUSALS: usize = 32;

/// The resolved policy: origins normalized once at startup, then consulted
/// per request.
#[derive(Clone, Debug, Default)]
pub struct AccessPolicy {
    /// Normalized extra origins, lowercase scheme and host, no trailing slash.
    allowed: Vec<String>,
    /// Origins already reported as refused, so the log carries one line per
    /// mistake rather than one per request.
    reported: Arc<Mutex<HashSet<String>>>,
    /// The pairing token, or `None` under `--no-token`. Minted per process,
    /// so restarting the engine invalidates whatever anyone copied.
    token: Option<Arc<String>>,
}

impl AccessPolicy {
    /// Resolve CLI options into a policy, rejecting values that could never
    /// match an `Origin` header rather than silently never matching one.
    pub fn new(opts: &AccessOptions) -> Result<Self, String> {
        let mut allowed = Vec::new();
        for raw in &opts.allow_origin {
            let origin = normalize_origin(raw)?;
            if !allowed.contains(&origin) {
                allowed.push(origin);
            }
        }
        Ok(Self {
            allowed,
            reported: Arc::new(Mutex::new(HashSet::new())),
            token: (!opts.no_token).then(|| Arc::new(mint_token())),
        })
    }

    /// Origins the user named, for the startup banner.
    pub fn extra_origins(&self) -> &[String] {
        &self.allowed
    }

    /// The pairing token, or `None` under `--no-token`.
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref().map(String::as_str)
    }

    /// True when a page on `origin` has to present the token as well as be
    /// allowlisted.
    ///
    /// Loopback is exempt because a UI on `http://localhost:4173` is one the
    /// user started; the webview is exempt because the extension has no way
    /// to be handed a token and is the surface this must not regress.
    fn needs_token(&self, origin: &str) -> bool {
        self.token.is_some() && !is_webview_origin(origin) && !is_loopback_origin(origin)
    }

    /// True when a page on `origin` may read this server's responses.
    pub fn allows(&self, origin: &str) -> bool {
        if is_webview_origin(origin) {
            return true;
        }
        match normalize_origin(origin) {
            Ok(normalized) => self.allowed.contains(&normalized),
            Err(_) => false,
        }
    }

    /// The CORS layer for this policy.
    ///
    /// Replaces `CorsLayer::permissive()`. The method and header lists are
    /// the ones the API actually uses — `POST` for `/api/diff`, `/api/scope`
    /// and friends, `DELETE` for leaving diff mode, `content-type` for their
    /// JSON bodies — rather than mirroring whatever the request asked for.
    pub fn cors_layer(&self) -> CorsLayer {
        let policy = self.clone();
        CorsLayer::new()
            .allow_origin(AllowOrigin::predicate(move |origin: &HeaderValue, _| {
                let Ok(origin) = origin.to_str() else {
                    return false;
                };
                if policy.allows(origin) {
                    return true;
                }
                policy.report_refusal(origin);
                false
            }))
            .allow_methods([Method::GET, Method::POST, Method::DELETE, Method::OPTIONS])
            .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION])
    }
}

impl AccessPolicy {
    /// Wrap `app` in the full access stack, in the one order that works.
    ///
    /// CORS has to sit *outside* the token gate: a 401 that carries no
    /// `access-control-allow-origin` is unreadable to the page that caused
    /// it, so the UI would see an indistinguishable network error and report
    /// "refused origin" for what is really "missing token". Exposed as one
    /// call so neither server can get the order wrong.
    pub fn apply(&self, app: axum::Router) -> axum::Router {
        app.layer(axum::middleware::from_fn_with_state(
            self.clone(),
            require_token,
        ))
        .layer(self.cors_layer())
    }

    /// Say once, on stderr, that a page was refused.
    ///
    /// A CORS refusal reaches the page as an indistinguishable network
    /// error, so without this the only symptom of a missing `--allow-origin`
    /// is a UI that never loads. The server is the one component that knows
    /// what actually happened.
    fn report_refusal(&self, origin: &str) {
        let Ok(mut reported) = self.reported.lock() else {
            return;
        };
        if reported.len() >= MAX_REPORTED_REFUSALS || !reported.insert(origin.to_string()) {
            return;
        }
        eprintln!("   ⚠ Refused a browser request from {origin}");
        eprintln!("     Restart with --allow-origin {origin} if that page is yours.");
    }
}

/// `GET /api/hello` — the one endpoint any origin may read.
///
/// Exists for UI-034's error messages. From a browser, "nao refused this
/// origin" and "that port is a `python3 -m http.server`" are the same opaque
/// network error: neither sends `access-control-allow-origin`, and the page
/// cannot see why. The connect screen has to tell them apart, because only
/// one of them has a remedy the user can act on (`--allow-origin`).
///
/// So this route answers everyone, and carries nothing worth stealing: what
/// the server is, which mode it is in, and whether it will want a token.
/// Registered *outside* the allowlist and the token gate — that is the whole
/// point of it — which is why it is a route factory rather than a handler.
pub fn hello_route(
    policy: &AccessPolicy,
    mode: &'static str,
    agent_spawn: bool,
) -> axum::routing::MethodRouter {
    let token_required = policy.token.is_some();
    axum::routing::get(move || async move {
        (
            [(header::CONTENT_TYPE, "application/json")],
            // `version` is the crate version; `commit` is what actually
            // distinguishes one build from the next, since the version does
            // not move between rebuilds. The UI shows both so "am I talking
            // to the engine I just installed" is answerable by looking.
            format!(
                r#"{{"server":"nao","mode":"{mode}","token_required":{token_required},"agent_spawn":{agent_spawn},"version":"{version}","commit":"{commit}"}}"#,
                version = env!("CARGO_PKG_VERSION"),
                commit = env!("NAO_GIT_COMMIT"),
            ),
        )
    })
    .layer(CorsLayer::permissive())
}

/// Reject a request from a token-requiring origin that did not bring one.
///
/// Requests with no `Origin` header pass straight through: they are
/// same-origin browser requests or non-browser clients, and neither is what
/// this gate is about. A page cannot forge the header, which is the whole
/// reason it can be trusted to select who gets asked.
async fn require_token(State(policy): State<AccessPolicy>, req: Request, next: Next) -> Response {
    let Some(expected) = policy.token() else {
        return next.run(req).await;
    };
    let origin = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let Some(origin) = origin else {
        return next.run(req).await;
    };
    if !policy.needs_token(&origin) {
        return next.run(req).await;
    }
    match presented_token(&req) {
        Some(given) if tokens_match(&given, expected) => next.run(req).await,
        Some(_) => token_error("token_invalid", "That pairing token is not this server's."),
        None => token_error(
            "token_required",
            "This origin needs the pairing token printed in the nao startup banner.",
        ),
    }
}

/// A 401 the UI can branch on without string-matching prose.
///
/// JSON rather than plain text because UI-034 shows a different screen for a
/// missing token than for a refused origin, and a refused origin never
/// produces a status at all — it fails as a network error — so the two are
/// only confusable if this body is unparseable.
fn token_error(code: &'static str, detail: &'static str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::CONTENT_TYPE, "application/json")],
        format!(
            r#"{{"error":"{code}","detail":"{detail} Send it as `Authorization: Bearer <token>` or `?{TOKEN_QUERY_PARAM}=<token>`."}}"#
        ),
    )
        .into_response()
}

/// The token this request carries, from either accepted place.
fn presented_token(req: &Request) -> Option<String> {
    let from_header = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
        .map(|(_, token)| token.trim().to_string());
    if from_header.is_some() {
        return from_header;
    }
    req.uri().query()?.split('&').find_map(|pair| {
        pair.split_once('=')
            .filter(|(k, _)| *k == TOKEN_QUERY_PARAM)
            .map(|(_, v)| v.to_string())
    })
}

/// Compare in time independent of how many leading bytes match, so the
/// server does not leak the token one byte at a time to a caller that can
/// measure it.
fn tokens_match(given: &str, expected: &str) -> bool {
    if given.len() != expected.len() {
        return false;
    }
    given
        .bytes()
        .zip(expected.bytes())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

/// True for an origin whose host is this machine.
///
/// The host has to *parse* as a loopback IP, or be exactly `localhost`.
/// A prefix test would be a hole: `127.0.0.1.evil.example` is a name anyone
/// can register and point wherever they like, and it starts with `127.`.
/// More generally a hostname that merely resolves to 127.0.0.1 is not
/// loopback for this purpose — DNS is attacker-controllable, and the point
/// of the exemption is "the user started this themselves".
fn is_loopback_origin(origin: &str) -> bool {
    let Some((scheme, rest)) = origin.split_once("://") else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return false;
    }
    let host = match rest.strip_prefix('[') {
        // IPv6 literal: `[::1]:4173`.
        Some(after) => match after.split_once(']') {
            Some((inner, _)) => inner,
            None => return false,
        },
        None => rest.split(':').next().unwrap_or(""),
    };
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// A fresh pairing token, 128 bits rendered as hex.
///
/// Read from the OS entropy pool where there is one. The fallback exists so
/// a missing `/dev/urandom` degrades to a weak token rather than a panic on
/// startup — but it hashes several independent sources so it is not
/// guessable from the clock alone.
fn mint_token() -> String {
    if let Some(bytes) = os_entropy() {
        return hex(&bytes);
    }
    let mut hasher = blake3::Hasher::new();
    hasher.update(&std::process::id().to_le_bytes());
    hasher.update(
        &std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
            .to_le_bytes(),
    );
    let heap = Box::new(0u8);
    hasher.update(&(Box::into_raw(heap) as usize).to_le_bytes());
    hex(&hasher.finalize().as_bytes()[..16])
}

/// 16 bytes from the OS entropy pool, or `None` where there isn't one.
fn os_entropy() -> Option<[u8; 16]> {
    use std::io::Read;
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .ok()?
        .read_exact(&mut bytes)
        .ok()?;
    Some(bytes)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Print the pairing token, once, where it can be copied.
///
/// Deliberately the only place it is ever written: it must not reach the
/// request log, the refusal diagnostic, or an error body.
pub fn print_pairing_token(policy: &AccessPolicy) {
    match policy.token() {
        Some(token) => {
            eprintln!("   Pairing token:  {token}");
            eprintln!("                   needed only by non-loopback origins; new every restart");
        }
        None => eprintln!("   Pairing token:  disabled (--no-token)"),
    }
}

/// Print what the policy allows beyond the defaults.
///
/// Only the user-supplied origins are listed: same-origin and the webview
/// are always on and saying so every launch would bury the line that
/// matters. An origin nobody expected is the thing worth seeing.
pub fn print_allowed_origins(policy: &AccessPolicy) {
    if policy.extra_origins().is_empty() {
        eprintln!("   Origins:        same-origin + vscode-webview:// only");
        return;
    }
    eprintln!("   Origins:        same-origin + vscode-webview://, plus:");
    for origin in policy.extra_origins() {
        eprintln!("                   ⚠ {origin} may read this server's responses");
    }
}

/// True for any `vscode-webview://…` origin.
pub(super) fn is_webview_origin(origin: &str) -> bool {
    origin.len() > WEBVIEW_SCHEME.len()
        && origin[..WEBVIEW_SCHEME.len()].eq_ignore_ascii_case(WEBVIEW_SCHEME)
}

/// Normalize an origin to the form an `Origin` header carries:
/// `scheme://host[:port]`, lowercase, no path and no trailing slash.
///
/// Written by hand rather than pulled from a URL crate because the accepted
/// grammar here is deliberately narrower than a URL: an origin with a path,
/// a userinfo section or a non-http scheme is a mistake worth reporting at
/// startup, not something to canonicalize into silence.
fn normalize_origin(raw: &str) -> Result<String, String> {
    let raw = raw.trim().trim_end_matches('/');
    let (scheme, rest) = raw
        .split_once("://")
        .ok_or_else(|| format!("not an origin (expected scheme://host): {raw}"))?;

    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(format!("origin scheme must be http or https: {raw}"));
    }
    if rest.is_empty() {
        return Err(format!("origin has no host: {raw}"));
    }
    if rest.contains('/') {
        return Err(format!("origin must have no path: {raw}"));
    }
    if rest.contains('@') || rest.contains('?') || rest.contains('#') {
        return Err(format!("origin must be scheme://host[:port] only: {raw}"));
    }

    Ok(format!("{scheme}://{}", rest.to_ascii_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request, routing::get, Router};
    use tower::ServiceExt;

    fn policy(origins: &[&str]) -> AccessPolicy {
        AccessPolicy::new(&AccessOptions {
            allow_origin: origins.iter().map(|s| s.to_string()).collect(),
            no_token: true,
        })
        .expect("test origins should be valid")
    }

    /// Same, but with the pairing token in force.
    fn tokened_policy(origins: &[&str]) -> AccessPolicy {
        AccessPolicy::new(&AccessOptions {
            allow_origin: origins.iter().map(|s| s.to_string()).collect(),
            no_token: false,
        })
        .expect("test origins should be valid")
    }

    fn app(policy: &AccessPolicy) -> Router {
        policy.apply(Router::new().route("/api/graph", get(|| async { "{}" })))
    }

    async fn allow_origin_header(policy: &AccessPolicy, req: Request<Body>) -> Option<String> {
        let resp = app(policy).oneshot(req).await.unwrap();
        resp.headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .map(|v| v.to_str().unwrap().to_string())
    }

    fn get_with_origin(origin: &str) -> Request<Body> {
        Request::builder()
            .uri("/api/graph")
            .header(header::ORIGIN, origin)
            .body(Body::empty())
            .unwrap()
    }

    fn preflight(origin: &str) -> Request<Body> {
        Request::builder()
            .method(Method::OPTIONS)
            .uri("/api/graph")
            .header(header::ORIGIN, origin)
            .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn cors_refuses_an_unknown_origin() {
        let p = policy(&[]);
        let got = allow_origin_header(&p, get_with_origin("https://evil.example")).await;
        assert_eq!(got, None);
    }

    #[tokio::test]
    async fn cors_allows_the_vscode_webview_with_no_flag() {
        let p = policy(&[]);
        let origin = "vscode-webview://1a2b3c";
        let got = allow_origin_header(&p, get_with_origin(origin)).await;
        assert_eq!(got.as_deref(), Some(origin));
    }

    #[tokio::test]
    async fn cors_allows_an_explicitly_allowed_origin() {
        let p = policy(&["http://localhost:4173"]);
        let got = allow_origin_header(&p, get_with_origin("http://localhost:4173")).await;
        assert_eq!(got.as_deref(), Some("http://localhost:4173"));
    }

    #[tokio::test]
    async fn cors_does_not_allow_a_sibling_port() {
        let p = policy(&["http://localhost:4173"]);
        let got = allow_origin_header(&p, get_with_origin("http://localhost:4174")).await;
        assert_eq!(got, None);
    }

    #[tokio::test]
    async fn cors_answers_preflight_only_for_allowed_origins() {
        let p = policy(&["http://localhost:4173"]);

        let allowed = allow_origin_header(&p, preflight("http://localhost:4173")).await;
        assert_eq!(allowed.as_deref(), Some("http://localhost:4173"));

        let refused = allow_origin_header(&p, preflight("https://evil.example")).await;
        assert_eq!(refused, None);
    }

    #[tokio::test]
    async fn cors_leaves_originless_requests_alone() {
        // curl and agents send no Origin; CORS is a browser policy and must
        // not turn into a half-hearted authentication check here.
        let p = policy(&[]);
        let req = Request::builder()
            .uri("/api/graph")
            .body(Body::empty())
            .unwrap();
        let resp = app(&p).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
    }

    #[test]
    fn cors_origin_normalization_is_case_and_slash_insensitive() {
        let p = policy(&["HTTP://Localhost:4173/"]);
        assert!(p.allows("http://localhost:4173"));
        assert_eq!(p.extra_origins(), ["http://localhost:4173"]);
    }

    #[test]
    fn cors_rejects_values_that_could_never_match_an_origin_header() {
        for bad in [
            "localhost:4173",
            "ftp://example.com",
            "https://example.com/app",
            "http://",
        ] {
            assert!(
                AccessPolicy::new(&AccessOptions {
                    allow_origin: vec![bad.to_string()],
                    no_token: true,
                })
                .is_err(),
                "{bad} should be rejected"
            );
        }
    }

    // ----------------------------------------------------------------
    //  Pairing token (SRV-009)
    // ----------------------------------------------------------------

    async fn status_for(policy: &AccessPolicy, uri: &str, origin: &str, auth: Option<&str>) -> u16 {
        let mut req = Request::builder().uri(uri).header(header::ORIGIN, origin);
        if let Some(auth) = auth {
            req = req.header(header::AUTHORIZATION, auth);
        }
        app(policy)
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
            .as_u16()
    }

    #[tokio::test]
    async fn token_is_required_by_a_remote_allowlisted_origin() {
        let p = tokened_policy(&["https://example.com"]);
        let status = status_for(&p, "/api/graph", "https://example.com", None).await;
        assert_eq!(status, 401);
    }

    #[tokio::test]
    async fn token_in_the_authorization_header_is_accepted() {
        let p = tokened_policy(&["https://example.com"]);
        let auth = format!("Bearer {}", p.token().unwrap());
        let status = status_for(&p, "/api/graph", "https://example.com", Some(&auth)).await;
        assert_eq!(status, 200);
    }

    #[tokio::test]
    async fn token_in_the_query_string_is_accepted() {
        // EventSource can't set headers, so the SSE stream depends on this.
        let p = tokened_policy(&["https://example.com"]);
        let uri = format!("/api/graph?token={}", p.token().unwrap());
        let status = status_for(&p, &uri, "https://example.com", None).await;
        assert_eq!(status, 200);
    }

    #[tokio::test]
    async fn token_that_is_wrong_is_refused_in_both_forms() {
        let p = tokened_policy(&["https://example.com"]);
        let header_form =
            status_for(&p, "/api/graph", "https://example.com", Some("Bearer nope")).await;
        assert_eq!(header_form, 401);

        let query_form = status_for(&p, "/api/graph?token=nope", "https://example.com", None).await;
        assert_eq!(query_form, 401);
    }

    #[tokio::test]
    async fn token_is_not_asked_of_loopback_or_the_webview() {
        let p = tokened_policy(&["http://localhost:4173", "http://127.0.0.1:8080"]);
        for origin in [
            "http://localhost:4173",
            "http://127.0.0.1:8080",
            "vscode-webview://1a2b3c",
        ] {
            assert_eq!(
                status_for(&p, "/api/graph", origin, None).await,
                200,
                "{origin} should not be asked for a token"
            );
        }
    }

    #[tokio::test]
    async fn token_is_not_asked_of_same_origin_requests() {
        // No Origin header: the page the engine served, or a non-browser
        // client. Neither is what the token is protecting against.
        let p = tokened_policy(&[]);
        let req = Request::builder()
            .uri("/api/graph")
            .body(Body::empty())
            .unwrap();
        let resp = app(&p).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn token_refusal_still_carries_cors_headers() {
        // Without them the page reads the 401 as a network error and can't
        // tell "needs a token" from "origin refused" — the distinction
        // UI-034's error messages are built on.
        let p = tokened_policy(&["https://example.com"]);
        let got = allow_origin_header(&p, get_with_origin("https://example.com")).await;
        assert_eq!(got.as_deref(), Some("https://example.com"));
    }

    #[tokio::test]
    async fn token_disabled_lets_a_remote_allowlisted_origin_straight_through() {
        let p = policy(&["https://example.com"]);
        assert!(p.token().is_none());
        let status = status_for(&p, "/api/graph", "https://example.com", None).await;
        assert_eq!(status, 200);
    }

    #[test]
    fn token_is_fresh_every_process() {
        let a = tokened_policy(&[]).token().unwrap().to_string();
        let b = tokened_policy(&[]).token().unwrap().to_string();
        assert_ne!(a, b);
        assert_eq!(a.len(), 32, "128 bits of hex");
    }

    #[test]
    fn token_loopback_test_accepts_only_literal_local_hosts() {
        for local in [
            "http://localhost:4173",
            "http://127.0.0.1",
            "https://127.9.9.9:1",
            "http://[::1]:5173",
        ] {
            assert!(is_loopback_origin(local), "{local}");
        }
        for remote in [
            "https://example.com",
            // Resolves to 127.0.0.1 on purpose; DNS is not a boundary.
            "http://localhost.evil.example",
            "http://127.0.0.1.evil.example",
        ] {
            assert!(!is_loopback_origin(remote), "{remote}");
        }
    }
}
