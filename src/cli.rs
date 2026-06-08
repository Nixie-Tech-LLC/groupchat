//! CLI client: builds control requests, auto-spawns the daemon, prints results.

use std::{path::Path, process::Stdio, time::Duration};

use anyhow::{anyhow, Context, Result};

use crate::{
    config::socket_path,
    control::{request, Request, Response},
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
                let tag = match e.kind {
                    crate::control::EventKind::Chat => "",
                    crate::control::EventKind::Join => "[join] ",
                    crate::control::EventKind::Call => "[call] ",
                    crate::control::EventKind::Resource => "[resource] ",
                    crate::control::EventKind::System => "[system] ",
                };
                println!("{}{}: {}", tag, e.nick, e.text);
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
