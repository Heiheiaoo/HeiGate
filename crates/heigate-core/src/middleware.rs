//! HTTP guards for a local-only desktop service: conservative security
//! headers and a local-origin policy that keeps web pages running in the
//! user's browser from talking to the management API.
//!
//! Ported one-to-one from the Go `internal/middleware` package; the security
//! behaviour must not drift.

use std::net::{IpAddr, SocketAddr};

use axum::{
    extract::{ConnectInfo, Request, State},
    http::{header, HeaderMap, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

/// Must be present on every state-changing management request. Browsers
/// cannot attach a custom header to a cross-origin request without a CORS
/// preflight, and HeiGate never answers preflights, so a malicious page
/// cannot forge such a request.
pub const REQUEST_HEADER: &str = "X-HeiGate-Request";

/// Strict content security policy suited to the embedded single-page UI.
/// Nothing is loaded from the network.
const CSP: &str = "default-src 'self'; \
script-src 'self'; \
style-src 'self' 'unsafe-inline'; \
img-src 'self' data: blob:; \
font-src 'self' data:; \
connect-src 'self'; \
worker-src 'self' blob:; \
frame-ancestors 'none'; \
base-uri 'self'; \
form-action 'self'";

/// Sets the security headers on every response.
pub async fn security_headers(req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    let headers = res.headers_mut();
    headers.insert(header::CONTENT_SECURITY_POLICY, CSP.parse().unwrap());
    headers.insert("x-content-type-options", "nosniff".parse().unwrap());
    headers.insert("x-frame-options", "DENY".parse().unwrap());
    headers.insert("referrer-policy", "no-referrer".parse().unwrap());
    headers.insert("cross-origin-opener-policy", "same-origin".parse().unwrap());
    headers.insert("cross-origin-resource-policy", "same-origin".parse().unwrap());
    res
}

/// Rejects connections that did not originate on this machine and requests
/// whose `Host` header does not name a loopback address. The second check
/// defeats DNS rebinding: a page on `attacker.example` that resolves to
/// `127.0.0.1` still arrives with `Host: attacker.example`.
pub async fn loopback_only(req: Request, next: Next) -> Response {
    if let Some(rejected) = reject_non_local(&req) {
        return rejected;
    }
    next.run(req).await
}

/// The browser-facing guard for the management API. Combines `loopback_only`
/// with an Origin / Sec-Fetch-Site check and requires [`REQUEST_HEADER`] on
/// every non-idempotent request.
pub async fn same_origin_only(req: Request, next: Next) -> Response {
    if let Some(rejected) = reject_non_local(&req) {
        return rejected;
    }

    let headers = req.headers();

    if let Some(origin) = header_str(headers, "origin") {
        if !origin.is_empty() && origin != "null" && !origin_is_loopback(origin) {
            return forbidden("cross-origin requests are not allowed");
        }
    }

    match header_str(headers, "sec-fetch-site").map(|s| s.to_ascii_lowercase()) {
        Some(site) if site == "cross-site" || site == "same-site" => {
            return forbidden("cross-origin requests are not allowed");
        }
        _ => {}
    }

    match *req.method() {
        Method::GET | Method::HEAD | Method::OPTIONS => {}
        _ => {
            let present = header_str(headers, REQUEST_HEADER)
                .map(|v| !v.trim().is_empty())
                .unwrap_or(false);
            if !present {
                return forbidden(&format!("missing {REQUEST_HEADER} header"));
            }
        }
    }

    next.run(req).await
}

/// The gateway credential, injected as state for [`bearer_auth`].
#[derive(Clone)]
pub struct GatewayKey(pub String);

/// Accepts the gateway key via `Authorization: Bearer <key>` or
/// `x-api-key: <key>`, matching what the Anthropic and OpenAI SDKs send.
pub async fn bearer_auth(State(GatewayKey(key)): State<GatewayKey>, req: Request, next: Next) -> Response {
    let key = key.trim().to_string();
    if key.is_empty() {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "gateway key is not configured" })),
        )
            .into_response();
    }

    let headers = req.headers();
    let mut provided = header_str(headers, "x-api-key")
        .map(|v| v.trim().to_string())
        .unwrap_or_default();
    if provided.is_empty() {
        if let Some(auth) = header_str(headers, header::AUTHORIZATION.as_str()) {
            let auth = auth.trim();
            if auth.len() > 7 && auth[..7].eq_ignore_ascii_case("Bearer ") {
                provided = auth[7..].trim().to_string();
            }
        }
    }

    let matches: bool =
        subtle::ConstantTimeEq::ct_eq(provided.as_bytes(), key.as_bytes()).into();
    let ok = !provided.is_empty() && matches;
    if !ok {
        return (
            StatusCode::UNAUTHORIZED,
            [(
                header::WWW_AUTHENTICATE,
                "Bearer realm=\"HeiGate\"",
            )],
            Json(json!({
                "type": "error",
                "error": {
                    "type": "authentication_error",
                    "message": "invalid HeiGate gateway API key"
                }
            })),
        )
            .into_response();
    }

    next.run(req).await
}

/// Aborts the request when it is not local; returns the rejection response
/// when the request must not proceed.
fn reject_non_local(req: &Request) -> Option<Response> {
    let remote_is_loopback = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip().is_loopback())
        .unwrap_or(false);
    if !remote_is_loopback {
        return Some(forbidden("HeiGate only accepts local connections"));
    }

    let host = header_str(req.headers(), header::HOST.as_str()).unwrap_or_default();
    if !is_loopback_host(&host) {
        return Some(forbidden("unexpected Host header"));
    }
    None
}

fn forbidden(message: &str) -> Response {
    (StatusCode::FORBIDDEN, Json(json!({ "error": message }))).into_response()
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// Parses an `Origin` header value (`scheme://host[:port]`) and reports
/// whether its host is loopback.
fn origin_is_loopback(origin: &str) -> bool {
    let rest = match origin.split_once("://") {
        Some((_, rest)) => rest,
        None => origin,
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    is_loopback_host(host)
}

/// Reports whether a `host[:port]` value names a loopback address.
pub fn is_loopback_host(hostport: &str) -> bool {
    let hostport = hostport.trim();
    if hostport.is_empty() {
        return false;
    }
    let host = split_host(hostport);
    let host = host.trim().trim_matches(|c| c == '[' || c == ']').to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    host.parse::<IpAddr>().map(|ip| ip.is_loopback()).unwrap_or(false)
}

/// Splits `host[:port]` into the host part. Handles IPv6 literals in brackets
/// and unbracketed bare IPv6 addresses.
fn split_host(hostport: &str) -> String {
    if let Some(end) = hostport.strip_prefix('[').and_then(|rest| rest.find(']')) {
        return hostport[1..=end].to_string();
    }
    match hostport.rsplit_once(':') {
        Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) => host.to_string(),
        _ => hostport.to_string(),
    }
}

#[cfg(test)]
mod tests {
    //! Ports `internal/middleware/middleware_test.go`. The cases and expected
    //! status codes must stay in lock-step with the Go originals.

    use super::*;
    use axum::{body::Body, middleware, routing::get, Router};
    use tower::ServiceExt;

    async fn call(
        app: Router,
        method: Method,
        path: &str,
        remote: &str,
        host: Option<&str>,
        headers: &[(&str, &str)],
    ) -> StatusCode {
        let mut builder = Request::builder().method(method).uri(path);
        for &(name, value) in headers {
            builder = builder.header(name, value);
        }
        if let Some(host) = host {
            builder = builder.header(header::HOST.as_str(), host);
        }
        let mut req = builder.body(Body::empty()).unwrap();
        req.extensions_mut()
            .insert(ConnectInfo(remote.parse::<SocketAddr>().unwrap()));
        app.oneshot(req).await.unwrap().status()
    }

    fn loopback_router() -> Router {
        Router::new()
            .route("/ping", get(|| async { "pong" }))
            .route("/write", axum::routing::post(|| async { "written" }))
            .layer(middleware::from_fn(loopback_only))
    }

    fn same_origin_router() -> Router {
        Router::new()
            .route("/ping", get(|| async { "pong" }))
            .route("/write", axum::routing::post(|| async { "written" }))
            .layer(middleware::from_fn(same_origin_only))
    }

    fn bearer_router(key: &str) -> Router {
        Router::new()
            .route("/ping", get(|| async { "pong" }))
            .route("/write", axum::routing::post(|| async { "written" }))
            .layer(middleware::from_fn_with_state(
                GatewayKey(key.to_string()),
                bearer_auth,
            ))
    }

    #[tokio::test]
    async fn loopback_only_rejects_non_local() {
        let r = loopback_router();
        assert_eq!(call(r.clone(), Method::GET, "/ping", "127.0.0.1:5000", Some("127.0.0.1:8080"), &[]).await, StatusCode::OK);
        assert_eq!(call(r.clone(), Method::GET, "/ping", "[::1]:5000", Some("localhost:8080"), &[]).await, StatusCode::OK);
        assert_eq!(call(r.clone(), Method::GET, "/ping", "127.0.0.1:5000", Some("[::1]:8080"), &[]).await, StatusCode::OK);
        assert_eq!(call(r.clone(), Method::GET, "/ping", "192.168.1.9:5000", Some("127.0.0.1:8080"), &[]).await, StatusCode::FORBIDDEN);
        assert_eq!(call(r.clone(), Method::GET, "/ping", "127.0.0.1:5000", Some("attacker.example:8080"), &[]).await, StatusCode::FORBIDDEN);
        assert_eq!(call(r, Method::GET, "/ping", "127.0.0.1:5000", None, &[]).await, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn same_origin_only_guards_browser_requests() {
        let r = same_origin_router();
        let local = Some("127.0.0.1:8080");

        assert_eq!(call(r.clone(), Method::GET, "/ping", "127.0.0.1:1", local, &[]).await, StatusCode::OK);
        assert_eq!(
            call(r.clone(), Method::GET, "/ping", "127.0.0.1:1", local, &[("origin", "http://127.0.0.1:8080"), ("sec-fetch-site", "same-origin")]).await,
            StatusCode::OK
        );
        assert_eq!(
            call(r.clone(), Method::GET, "/ping", "127.0.0.1:1", local, &[("origin", "http://localhost:5173")]).await,
            StatusCode::OK
        );
        assert_eq!(
            call(r.clone(), Method::GET, "/ping", "127.0.0.1:1", local, &[("origin", "https://evil.example")]).await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            call(r.clone(), Method::GET, "/ping", "127.0.0.1:1", local, &[("sec-fetch-site", "cross-site")]).await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            call(r.clone(), Method::POST, "/write", "127.0.0.1:1", local, &[("content-type", "text/plain")]).await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            call(r.clone(), Method::POST, "/write", "127.0.0.1:1", local, &[(REQUEST_HEADER, "1")]).await,
            StatusCode::OK
        );
        assert_eq!(
            call(r, Method::POST, "/write", "10.0.0.2:1", local, &[(REQUEST_HEADER, "1")]).await,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn bearer_auth_checks_keys() {
        let r = bearer_router("hg_secret");
        let local = Some("127.0.0.1:8080");
        assert_eq!(call(r.clone(), Method::GET, "/ping", "127.0.0.1:1", local, &[]).await, StatusCode::UNAUTHORIZED);
        assert_eq!(call(r.clone(), Method::GET, "/ping", "127.0.0.1:1", local, &[("authorization", "Bearer wrong")]).await, StatusCode::UNAUTHORIZED);
        assert_eq!(call(r.clone(), Method::GET, "/ping", "127.0.0.1:1", local, &[("authorization", "Bearer hg_secret")]).await, StatusCode::OK);
        assert_eq!(call(r.clone(), Method::GET, "/ping", "127.0.0.1:1", local, &[("authorization", "bearer hg_secret")]).await, StatusCode::OK);
        assert_eq!(call(r, Method::GET, "/ping", "127.0.0.1:1", local, &[("x-api-key", "hg_secret")]).await, StatusCode::OK);

        let empty = bearer_router("");
        assert_eq!(call(empty, Method::GET, "/ping", "127.0.0.1:1", local, &[("x-api-key", "x")]).await, StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn security_headers_are_set() {
        let app = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(middleware::from_fn(security_headers));
        let res = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let csp = res
            .headers()
            .get(header::CONTENT_SECURITY_POLICY)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(csp.contains("frame-ancestors 'none'"));
        assert_eq!(
            res.headers().get("x-content-type-options").unwrap(),
            "nosniff"
        );
    }
}
