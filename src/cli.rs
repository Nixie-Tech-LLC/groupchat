//! CLI client: builds control requests, auto-spawns the daemon, prints results.

use std::{io::Write, path::Path, process::Stdio, time::Duration};

use anyhow::{anyhow, Context, Result};

use crate::{
    config::socket_path,
    control::{request, Event, EventKind, Request, Response},
};

/// Ensure a daemon is running for this home dir, spawning one if needed.
pub async fn ensure_daemon(home: &Path) -> Result<()> {
    let socket = socket_path(home);
    if request(&socket, &Request::Status).await.is_ok() {
        return Ok(());
    }

    let exe = std::env::current_exe().context("locate own executable")?;
    std::process::Command::new(exe)
        .arg("daemon")
        .env("GROUPCHAT_HOME", home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("spawn daemon")?;

    // Wait for the daemon to come online (it binds a relay before serving).
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        if request(&socket, &Request::Status).await.is_ok() {
            return Ok(());
        }
    }
    Err(anyhow!("daemon did not come online in time"))
}

/// Ensure the daemon is up, then send one request.
pub async fn client(home: &Path, req: Request) -> Result<Response> {
    ensure_daemon(home).await?;
    request(&socket_path(home), &req).await
}

/// Run a request and pretty-print the response for terminal users.
pub async fn run(home: &Path, req: Request) -> Result<()> {
    let resp = client(home, req).await?;
    print_response(resp);
    Ok(())
}

fn print_response(resp: Response) {
    match resp {
        Response::Ok { message } => {
            if let Some(m) = message {
                println!("{m}");
            } else {
                println!("ok");
            }
        }
        Response::Text { text } => println!("{text}"),
        Response::Status(s) => {
            println!("id:        {}", s.id);
            println!("nick:      {}", s.nick);
            println!("room:      {}", s.room);
            println!("online:    {} peer(s)", s.online_peers);
            println!("contacts:  {}", s.contacts);
            println!("resources: {}", s.resources);
        }
        Response::Events { events, last } => {
            for e in &events {
                print_event(e);
            }
            if events.is_empty() {
                println!("(no new messages)");
            } else {
                println!("--- last seq {last} ---");
            }
        }
        Response::Contacts { contacts } => {
            if contacts.is_empty() {
                println!("(no contacts)");
            }
            for c in contacts {
                println!("{}  {}", c.nick, c.id);
            }
        }
        Response::Who { mut peers } => {
            if peers.is_empty() {
                println!("(no peers seen yet)");
            }
            peers.sort_by_key(|p| (!p.online, p.nick.clone()));
            for p in peers {
                let dot = if p.online { "\u{25CF}" } else { "\u{25CB}" };
                let star = if p.is_contact { " \u{2713}contact" } else { "" };
                println!("{dot} {}  ({}){star}", p.nick, p.id);
            }
        }
        Response::Resources { resources } => {
            if resources.is_empty() {
                println!("(no resources shared)");
            }
            for r in resources {
                println!("{}  from {}\n    {}", r.label, r.from, r.ticket);
            }
        }
        Response::Error { message } => {
            eprintln!("error: {message}");
        }
    }
}

/// Short machine-readable name for an event kind (also used as a hook env var).
fn kind_str(k: &EventKind) -> &'static str {
    match k {
        EventKind::Chat => "chat",
        EventKind::Join => "join",
        EventKind::Call => "call",
        EventKind::Resource => "resource",
        EventKind::Presence => "presence",
        EventKind::System => "system",
    }
}

/// Print one event the way `log`/`watch` show it (🔔 marks direct ones).
fn print_event(e: &Event) {
    let tag = match e.kind {
        EventKind::Chat => "",
        EventKind::Join => "[join] ",
        EventKind::Call => "[call] ",
        EventKind::Resource => "[resource] ",
        EventKind::Presence => "[presence] ",
        EventKind::System => "[system] ",
    };
    let bell = if e.direct { "\u{1F514} " } else { "" };
    println!("{bell}{tag}{}: {}", e.nick, e.text);
}

/// Run a user hook for an event: the event fields are exported as environment
/// variables and the full event JSON is piped to the command's stdin. Detached
/// so a slow hook never stalls the watch loop.
fn run_hook(cmd: &str, e: &Event) {
    let json = serde_json::to_string(e).unwrap_or_default();
    let child = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .env("GROUPCHAT_EVENT_SEQ", e.seq.to_string())
        .env("GROUPCHAT_EVENT_KIND", kind_str(&e.kind))
        .env("GROUPCHAT_EVENT_NICK", &e.nick)
        .env("GROUPCHAT_EVENT_ID", &e.id)
        .env("GROUPCHAT_EVENT_TEXT", &e.text)
        .env("GROUPCHAT_EVENT_DIRECT", if e.direct { "true" } else { "false" })
        .env("GROUPCHAT_EVENT_TS", e.ts.to_string())
        .stdin(Stdio::piped())
        .spawn();
    match child {
        Ok(mut child) => {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(json.as_bytes());
            }
            // Reap in the background so we don't block or leave a zombie.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(err) => eprintln!("watch: hook failed to start: {err}"),
    }
}

/// Fire a desktop notification for an event (best-effort, platform-native).
fn desktop_notify(e: &Event) {
    let title = format!("groupchat: {}", e.nick);
    if cfg!(target_os = "macos") {
        let script = format!(
            "display notification {:?} with title {:?}",
            e.text, title
        );
        let _ = std::process::Command::new("osascript")
            .arg("-e")
            .arg(script)
            .spawn();
    } else {
        let _ = std::process::Command::new("notify-send")
            .arg(&title)
            .arg(&e.text)
            .spawn();
    }
}

/// Foreground notification runner: block on `chat_wait`, print each event, and
/// for matching events run a hook command and/or raise a desktop notification.
/// Loops forever (Ctrl-C to stop), reconnecting if the daemon restarts.
pub async fn watch(
    home: &Path,
    since: Option<u64>,
    direct_only: bool,
    exec: Option<String>,
    notify: bool,
    timeout_ms: u64,
) -> Result<()> {
    ensure_daemon(home).await?;
    let sock = socket_path(home);

    // Default: start from "now" so we don't replay the whole backlog.
    let mut cursor = match since {
        Some(n) => n,
        None => match request(&sock, &Request::Log { since: 0 }).await? {
            Response::Events { last, .. } => last,
            _ => 0,
        },
    };
    eprintln!("watching from seq {cursor} (Ctrl-C to stop)\u{2026}");

    loop {
        let resp = match request(&sock, &Request::Wait { since: cursor, timeout_ms }).await {
            Ok(r) => r,
            Err(e) => {
                // Daemon may have restarted; re-ensure and keep going.
                eprintln!("watch: {e}; reconnecting\u{2026}");
                tokio::time::sleep(Duration::from_millis(500)).await;
                let _ = ensure_daemon(home).await;
                continue;
            }
        };
        if let Response::Events { events, last } = resp {
            for e in &events {
                print_event(e);
                if !direct_only || e.direct {
                    if let Some(cmd) = &exec {
                        run_hook(cmd, e);
                    }
                    if notify {
                        desktop_notify(e);
                    }
                }
            }
            cursor = last.max(cursor);
        }
    }
}
