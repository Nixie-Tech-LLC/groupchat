//! `lait serve` — the local HTTP surface, and the browser's Layer-B client.
//!
//! The engine's contract has always been [`crate::control`]: a versioned,
//! hand-maintained imperative façade over the CRDT, spoken over a Unix socket or
//! a named pipe. Every client so far (CLI, TUI, MCP) is a local process, so that
//! transport cost them nothing. A browser cannot speak a named pipe. This module
//! is the *one* adapter that closes that gap — the same `Request`/`Response`
//! types, the same `Doorbell` stream, re-bound to a loopback TCP socket and SSE.
//!
//! Two things follow, and they are the whole design:
//!
//! **This is a supervisor, not a client.** The control channel is keyed by home,
//! so there is one daemon per space. A CLI invocation resolves exactly one store
//! and talks to exactly one daemon; the browser is a picker over *all* of them,
//! so it holds N. See [`spaces::Supervisor`].
//!
//! **The socket was the authentication.** Binding the same façade to a TCP port
//! removes the OS permission check that made auth unnecessary, and adds a caller
//! that never existed before: the web pages the user visits. See [`auth`].
//!
//! The browser is deliberately *not* a peer. It holds no key, has no entry in the
//! ACL, and is never invited: it is a lens on a device's replica, exactly like
//! the TUI was, and the device remains the only network identity.

pub mod auth;
pub mod spaces;

mod shell;

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    middleware::Next,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Redirect, Response,
    },
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use tokio::net::TcpListener;
use tokio_stream::wrappers::{errors::BroadcastStreamRecvError, BroadcastStream};
use tokio_stream::StreamExt;

use crate::control::{ErrorKind, Request};
use auth::{Guard, Refusal};
use spaces::Supervisor;

/// The default port. Fixed rather than ephemeral so the URL is predictable and
/// the `Origin` allowlist has something stable to name; a collision is reported
/// rather than silently worked around, because a `lait serve` that lands on a
/// *different* port than it was asked for is a footgun for anything that
/// bookmarked it.
pub const DEFAULT_PORT: u16 = 7717;

/// The cookie the browser trades its one-time URL token for.
const COOKIE: &str = "lait_token";

struct App {
    guard: Guard,
    sup: Supervisor,
}

/// Run the local server until interrupted.
pub async fn run(port: u16, open: bool) -> Result<()> {
    // Identity scoping, resolved once at startup — see `spaces::scope` for why
    // `$LAIT_HOME` is the axis that matters.
    let identity = crate::config::identity_dir()?;
    let self_contained = std::env::var_os("LAIT_HOME").is_some();
    let agents_base = crate::registry::agents_base(&crate::config::config_root()?);

    // Loopback only. Not `0.0.0.0`: that would hand the LAN an unauthenticated-
    // by-default view of every space on this machine, and the token is the only
    // thing that would stand between them and it.
    let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))
        .await
        .with_context(|| {
            format!("bind 127.0.0.1:{port} (is another `lait serve` already running?)")
        })?;
    let bound = listener.local_addr().context("read bound address")?;

    let token = mint_token();
    let app = Arc::new(App {
        guard: Guard::new(token.clone(), bound.port()),
        sup: Supervisor::new(identity, agents_base, self_contained),
    });

    let url = format!("http://127.0.0.1:{}/?token={}", bound.port(), token);
    println!("lait serve — your spaces at:\n  {url}");
    println!("(loopback only; this link carries a one-time token for this run)");
    if open {
        open_browser(&url);
    }

    axum::serve(listener, router(app)).await.context("serve")?;
    Ok(())
}

fn router(app: Arc<App>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/spaces", get(list_spaces))
        .route("/api/spaces/{id}/board", get(board))
        .route("/api/events", get(events))
        .layer(axum::middleware::from_fn_with_state(app.clone(), gate))
        .with_state(app)
}

/// A 32-byte hex token, minted per run and never persisted.
fn mint_token() -> String {
    let mut buf = [0u8; 32];
    getrandom::fill(&mut buf).expect("getrandom");
    data_encoding::HEXLOWER.encode(&buf)
}

/// The gate every request passes: rebinding guard first, credential second.
///
/// Ordering is deliberate. `check_origin` is what survives a successful rebind
/// (at which point the browser *will* hand over our cookie), so it must not be
/// reachable-past by anything the attacker controls. The token is checked only
/// once we already believe the request is addressed to us by a loopback name.
async fn gate(State(app): State<Arc<App>>, req: axum::extract::Request, next: Next) -> Response {
    let headers = req.headers();
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    if let Err(r) = app.guard.check_origin(host, origin) {
        return refuse(r);
    }

    // Three ways to present the token, one meaning. The query form exists only
    // for the opening navigation — `index` immediately trades it for the cookie
    // and redirects, so it never lingers in history or a Referer.
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let cookie = headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|c| auth::cookie_value(c, COOKIE));
    let query = req.uri().query().and_then(|q| query_param(q, "token"));
    let presented = bearer.or(cookie).or(query.as_deref());

    if let Err(r) = app.guard.check_token(presented) {
        return refuse(r);
    }
    next.run(req).await
}

fn refuse(r: Refusal) -> Response {
    let code = match r {
        Refusal::BadToken => StatusCode::UNAUTHORIZED,
        _ => StatusCode::FORBIDDEN,
    };
    (code, err_json(r.reason(), ErrorKind::Error)).into_response()
}

/// Errors go out in the same envelope `--json` emits, so a browser client and a
/// CLI client are reading one contract rather than two.
fn err_json(message: &str, error_kind: ErrorKind) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "kind": "error",
        "message": message,
        "error_kind": error_kind,
    }))
}

/// Minimal `application/x-www-form-urlencoded` lookup — one key, no allocation
/// beyond the hit. Avoids a query-string crate for a single parameter.
fn query_param(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == name).then(|| v.to_string())
    })
}

#[derive(Deserialize)]
struct IndexQuery {
    token: Option<String>,
}

/// The shell — and the one-time token handoff.
///
/// Arriving with `?token=` means this is the opening navigation: set the cookie
/// and redirect to a clean `/`. The token is then out of the URL bar, out of
/// history, and out of any `Referer` the page might later emit. `HttpOnly` keeps
/// it out of reach of script in our own page; `SameSite=Strict` keeps the browser
/// from attaching it to anyone else's request.
async fn index(State(_app): State<Arc<App>>, Query(q): Query<IndexQuery>) -> Response {
    if let Some(token) = q.token {
        let cookie = format!("{COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict");
        return ([(header::SET_COOKIE, cookie)], Redirect::to("/")).into_response();
    }
    axum::response::Html(shell::HTML).into_response()
}

async fn list_spaces(State(app): State<Arc<App>>) -> Response {
    Json(serde_json::json!({ "spaces": app.sup.list().await })).into_response()
}

#[derive(Deserialize)]
struct BoardQuery {
    project: Option<String>,
}

/// One space's board. Selecting a space is what attaches its daemon — this is
/// the first point at which anything is started.
async fn board(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Query(q): Query<BoardQuery>,
) -> Response {
    // `project: None` is legitimate: the daemon's choose-project chain resolves
    // the view (sole project / `project.default` / branch hint), so the picker
    // does not have to know a project before it can show a board.
    let req = Request::Board {
        project: q.project,
        project_hint: None,
    };
    match app.sup.request(&id, &req).await {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            err_json(&e.to_string(), ErrorKind::Error),
        )
            .into_response(),
    }
}

/// The doorbell multiplex: one `EventSource` over every attached space.
///
/// Carries dirty *flags*, never state — the browser re-reads the authoritative
/// projection for each dirty scope, exactly as the TUI does (UI.md §4.2). A
/// `Lagged` receiver is surfaced rather than hidden: the client's response is the
/// same rebaseline it already performs for `reset`/epoch changes (UI.md §4.1), so
/// dropping frames under load is recoverable by construction.
async fn events(
    State(app): State<Arc<App>>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let stream = BroadcastStream::new(app.sup.subscribe()).map(|r| {
        Ok(match r {
            Ok(sd) => Event::default()
                .event("doorbell")
                .json_data(sd)
                .unwrap_or_else(|_| Event::default().event("lagged").data("encode")),
            Err(BroadcastStreamRecvError::Lagged(n)) => {
                Event::default().event("lagged").data(n.to_string())
            }
        })
    });
    // Keep-alive so an idle space (no doorbells for minutes) doesn't look like a
    // dead connection to an intermediary or to the browser's own reconnect logic.
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// Best-effort browser launch. Failure is not an error: the URL is already on
/// stdout, which is the contract; opening a window is a courtesy.
fn open_browser(url: &str) {
    let spawned = if cfg!(windows) {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(url).spawn()
    };
    if let Err(e) = spawned {
        tracing::debug!(error = %e, "could not open a browser; use the printed URL");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_param_finds_only_an_exact_key() {
        assert_eq!(query_param("token=abc", "token"), Some("abc".into()));
        assert_eq!(
            query_param("a=1&token=abc&b=2", "token"),
            Some("abc".into())
        );
        assert_eq!(query_param("a=1", "token"), None);
        // A key that merely ends with ours must not match.
        assert_eq!(query_param("xtoken=abc", "token"), None);
        assert_eq!(query_param("", "token"), None);
    }

    #[test]
    fn minted_tokens_are_64_hex_chars_and_not_repeated() {
        let a = mint_token();
        let b = mint_token();
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b, "a per-run token must not be deterministic");
    }
}
