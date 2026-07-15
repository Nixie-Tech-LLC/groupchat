//! The space supervisor — identity scoping, and lazy per-space daemon attach.
//!
//! Every Layer-B client before this one spoke to exactly **one** daemon: the
//! control channel is keyed by home ([`crate::control::control_name`]), and a CLI
//! invocation resolves exactly one store. The browser is the first client that is
//! *global to the machine* — a spaces picker means holding several daemons at
//! once — so this module is the piece with no prior art in the codebase.
//!
//! Two invariants shape it.
//!
//! **Never spawn what you were not asked for.** [`list`] answers the picker by
//! probing (a short-timeout [`Request::Status`] that fails closed to `idle`),
//! never by starting anything: opening the browser must not wake every daemon a
//! user has ever registered. A space's daemon starts only when that space is
//! actually selected — see [`Supervisor::attach`].
//!
//! **Never cross an identity.** See [`scope`], which is the whole story.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use serde::Serialize;
use tokio::sync::{broadcast, Mutex};

use crate::control::{self, Doorbell, Request};
use crate::workspaces::{self, StorePresence, WorkspaceEntry};

/// A doorbell, tagged with the space it rang for.
///
/// The tab holds one `EventSource` over N attached spaces, so the space id is the
/// demultiplexing key. Flattened so the wire shape is a [`Doorbell`] plus one
/// field — the browser re-reads the authoritative projection for each dirty
/// scope exactly as the TUI does (UI.md §4.2); this is still a dirty *flag*, not
/// state.
#[derive(Debug, Clone, Serialize)]
pub struct SpaceDoorbell {
    pub space: String,
    #[serde(flatten)]
    pub doorbell: Doorbell,
}

/// One row of the spaces picker.
#[derive(Debug, Clone, Serialize)]
pub struct SpaceRow {
    /// Stable, opaque handle for URLs — see [`space_id`].
    pub id: String,
    /// The `ws_…` workspace id.
    pub workspace: String,
    /// Display name at last open (advisory — the catalog is authoritative).
    pub name: String,
    pub path: String,
    pub origin: String,
    pub last_opened: u64,
    /// `up` | `idle` | `missing`, exactly as `lait spaces` reports it.
    pub status: &'static str,
    pub projects: Vec<workspaces::ProjectBrief>,
}

/// A stable public id for a store path.
///
/// Derived rather than borrowed: the `ws_` id is not unique per *store* (the same
/// workspace can legitimately be bound at two paths), and the store path itself
/// is both unwieldy and a filesystem disclosure in a URL. blake3 is already in
/// the tree, and unlike [`crate::config::home_hash`] — whose `DefaultHasher` is
/// explicitly not stable across Rust releases, which is fine for the socket name
/// it exists for — this stays put across builds, so a bookmarked space URL keeps
/// resolving.
pub fn space_id(path: &str) -> String {
    let hash = blake3::hash(path.as_bytes());
    hash.to_hex()[..16].to_string()
}

/// Which registered spaces belong to `identity`.
///
/// **This function is the identity seam.** Getting it right depends on a fact
/// that is easy to invert: in lait, identity is *global by default*.
/// [`crate::config::identity_dir`] puts `secret.key` under the config root and
/// one key spans every repo-bound store — "like one `git` `user.email` across
/// many repos" — so N ordinary spaces are N daemons signing with the *same*
/// identity. Listing them side by side crosses nothing.
///
/// The exception is a **self-contained home**: `$LAIT_HOME` collapses identity
/// and store into one directory, giving that home its own `secret.key`. Named
/// agents are exactly that shape, living under [`crate::registry::agents_base`],
/// and [`crate::registry`] isolates them deliberately — "separate homes mean
/// separate `secret.key`s… one agent can't read another" — under a never-guess
/// invariant, because a wrong auto-attach is a cross-identity leak.
///
/// `workspaces.json` is a single global file that every daemon open upserts into
/// (`node.rs`), so it holds *both* kinds. A picker that rendered it verbatim
/// would silently offer an agent's spaces alongside your own, and acting in one
/// would act as that agent. Hence:
///
/// - a **global** identity owns every registered store *except* the self-contained
///   homes under `agents_base`;
/// - a **self-contained** identity owns exactly its own home and nothing else.
///
/// SEAM: an identity switcher changes only the caller — it picks a different
/// `identity`/`self_contained` pair and calls this again. Scoping is decided
/// here and nowhere else, so the switcher never has to be threaded through the
/// router, the supervisor, or the endpoints.
pub fn scope<'a>(
    entries: &'a [WorkspaceEntry],
    identity: &Path,
    agents_base: &Path,
    self_contained: bool,
) -> Vec<&'a WorkspaceEntry> {
    entries
        .iter()
        .filter(|e| {
            let path = Path::new(&e.path);
            if self_contained {
                // $LAIT_HOME: this identity is its own store and owns only itself.
                same_path(path, identity)
            } else {
                // The global identity: everything except somebody else's home.
                !under(path, agents_base)
            }
        })
        .collect()
}

/// Path equality that survives the shapes these strings actually arrive in.
///
/// Registry paths are written by several call sites and compared against a value
/// derived from the environment, so they can differ in separator and — on
/// Windows, where the filesystem is case-insensitive — in case, while naming the
/// same directory. A false negative here would hide a user's own space from the
/// picker; a false positive cannot cross an identity, because `agents_base` is a
/// distinct subtree either way.
fn same_path(a: &Path, b: &Path) -> bool {
    normalize(a) == normalize(b)
}

fn under(path: &Path, base: &Path) -> bool {
    let (path, base) = (normalize(path), normalize(base));
    Path::new(&path).starts_with(Path::new(&base))
}

fn normalize(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let s = s.trim_end_matches('/').to_string();
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s
    }
}

/// Probe a store's daemon without starting one.
///
/// Mirrors `cli::workspace_status` — deliberately, so the browser and `lait
/// spaces` cannot disagree about what "up" means. The short timeout fails closed
/// to `idle`: a picker that hangs on a wedged daemon is worse than one that
/// under-reports it, and selecting the space will start it anyway.
async fn status(entry: &WorkspaceEntry) -> &'static str {
    if workspaces::presence(entry) == StorePresence::Missing {
        return "missing";
    }
    let up = tokio::time::timeout(
        Duration::from_millis(300),
        control::request(Path::new(&entry.path), &Request::Status),
    )
    .await
    .map(|r| r.is_ok())
    .unwrap_or(false);
    if up {
        "up"
    } else {
        "idle"
    }
}

/// A live attachment to one space's daemon: the task pumping its doorbells into
/// the shared fan-in. Dropping it aborts the pump.
struct Attached {
    home: PathBuf,
    pump: tokio::task::JoinHandle<()>,
}

impl Drop for Attached {
    fn drop(&mut self) {
        self.pump.abort();
    }
}

/// Holds the N daemons the browser is currently looking at.
pub struct Supervisor {
    identity: PathBuf,
    agents_base: PathBuf,
    self_contained: bool,
    attached: Mutex<HashMap<String, Arc<Attached>>>,
    doorbells: broadcast::Sender<SpaceDoorbell>,
}

impl Supervisor {
    pub fn new(identity: PathBuf, agents_base: PathBuf, self_contained: bool) -> Self {
        // Bounded: a lagging tab must not let the daemon's dirty-set pin memory.
        // A dropped frame is recoverable by construction — the receiver sees
        // `Lagged`, and the contract for that is the same `reset` rebaseline the
        // TUI already performs on an epoch change (UI.md §4.1).
        let (doorbells, _) = broadcast::channel(256);
        Self {
            identity,
            agents_base,
            self_contained,
            attached: Mutex::new(HashMap::new()),
            doorbells,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SpaceDoorbell> {
        self.doorbells.subscribe()
    }

    /// The spaces this identity owns, newest-first, each with a probed status.
    ///
    /// Probes run concurrently: sequential 300ms timeouts would make the picker's
    /// latency the *sum* of every idle space, which is exactly the case a user
    /// with a dozen registered spaces hits.
    pub async fn list(&self) -> Vec<SpaceRow> {
        let entries = workspaces::list();
        let scoped = scope(
            &entries,
            &self.identity,
            &self.agents_base,
            self.self_contained,
        );
        let mut set = tokio::task::JoinSet::new();
        for e in scoped {
            let e = e.clone();
            set.spawn(async move {
                SpaceRow {
                    id: space_id(&e.path),
                    workspace: e.workspace.clone(),
                    name: e.name.clone(),
                    path: e.path.clone(),
                    origin: e.origin.to_string(),
                    last_opened: e.last_opened,
                    status: status(&e).await,
                    projects: e.projects.clone(),
                }
            });
        }
        let mut rows = set.join_all().await;
        // `JoinSet` yields in completion order, so restore the registry's own
        // newest-first ordering rather than letting probe latency decide it.
        rows.sort_by_key(|r| std::cmp::Reverse(r.last_opened));
        rows
    }

    /// Resolve a public space id to its home, scoped to this identity.
    ///
    /// Resolution goes through [`scope`], so an id belonging to another identity
    /// is indistinguishable from one that does not exist — the browser cannot
    /// address an agent's space by guessing, only by being given a different
    /// identity to run under.
    pub fn resolve(&self, id: &str) -> Result<PathBuf> {
        let entries = workspaces::list();
        let scoped = scope(
            &entries,
            &self.identity,
            &self.agents_base,
            self.self_contained,
        );
        scoped
            .into_iter()
            .find(|e| space_id(&e.path) == id)
            .map(|e| PathBuf::from(&e.path))
            .ok_or_else(|| anyhow!("no such space"))
    }

    /// Ensure this space's daemon is up and its doorbells are flowing.
    ///
    /// Idempotent, and the *only* place a daemon is started: attaching is what
    /// selecting a space means. Returns the home so callers can round-trip it.
    pub async fn attach(&self, id: &str) -> Result<PathBuf> {
        let home = self.resolve(id)?;
        let mut attached = self.attached.lock().await;
        if let Some(a) = attached.get(id) {
            return Ok(a.home.clone());
        }
        crate::cli::ensure_daemon(&home).await?;

        let tx = self.doorbells.clone();
        let space = id.to_string();
        let pump_home = home.clone();
        let pump = tokio::spawn(async move {
            // `since: 0` asks for a full rebaseline, matching a fresh TUI attach.
            let mut sub = match control::subscribe(&pump_home, 0).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(space = %space, error = %e, "subscribe failed");
                    return;
                }
            };
            loop {
                match sub.next().await {
                    Ok(Some(doorbell)) => {
                        // Err only means "nobody listening" — the tab is closed.
                        // Keep pumping: it may come back, and the daemon is up
                        // regardless of whether anyone is watching.
                        let _ = tx.send(SpaceDoorbell {
                            space: space.clone(),
                            doorbell,
                        });
                    }
                    // EOF: the daemon stopped. Detaching here (rather than
                    // looping to reconnect) keeps the restart policy in one
                    // place — the next request re-attaches through
                    // `ensure_daemon`, which already owns the heal path.
                    Ok(None) => break,
                    Err(e) => {
                        tracing::warn!(space = %space, error = %e, "doorbell stream ended");
                        break;
                    }
                }
            }
        });

        attached.insert(
            id.to_string(),
            Arc::new(Attached {
                home: home.clone(),
                pump,
            }),
        );
        Ok(home)
    }

    /// Round-trip a request to a space's daemon, attaching it first if needed.
    pub async fn request(&self, id: &str, req: &Request) -> Result<control::Response> {
        let home = self.attach(id).await?;
        control::request(&home, req).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str) -> WorkspaceEntry {
        WorkspaceEntry {
            workspace: "ws_test".into(),
            name: "Test".into(),
            path: path.into(),
            origin: workspaces::Origin::Founded,
            host_nick: String::new(),
            last_opened: 0,
            projects: Vec::new(),
        }
    }

    #[test]
    fn global_identity_sees_repo_stores_but_not_agent_homes() {
        // The case that matters: workspaces.json is one global file holding both
        // kinds, and an agent home carries its own secret.key. Offering it in the
        // picker would let one click act as that agent.
        let entries = vec![
            entry("/home/u/proj-a/.lait"),
            entry("/home/u/.config/lait/agents/scout"),
            entry("/home/u/proj-b/.lait"),
        ];
        let scoped = scope(
            &entries,
            Path::new("/home/u/.config/lait"),
            Path::new("/home/u/.config/lait/agents"),
            false,
        );
        let paths: Vec<&str> = scoped.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["/home/u/proj-a/.lait", "/home/u/proj-b/.lait"]);
    }

    #[test]
    fn self_contained_identity_sees_only_itself() {
        // $LAIT_HOME (and every named agent) is its own identity: it must not be
        // shown its siblings, and must not be shown the global identity's stores.
        let entries = vec![
            entry("/home/u/proj-a/.lait"),
            entry("/home/u/.config/lait/agents/scout"),
            entry("/home/u/.config/lait/agents/other"),
        ];
        let scoped = scope(
            &entries,
            Path::new("/home/u/.config/lait/agents/scout"),
            Path::new("/home/u/.config/lait/agents"),
            true,
        );
        let paths: Vec<&str> = scoped.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["/home/u/.config/lait/agents/scout"]);
    }

    #[test]
    fn scoping_is_not_fooled_by_separator_or_case_drift() {
        // Registry paths are written by several call sites; on Windows the same
        // directory can arrive spelled differently. A false negative would hide a
        // user's own space, so normalize before comparing.
        let entries = vec![entry(r"C:\Users\U\proj\.lait")];
        let scoped = scope(
            &entries,
            Path::new("C:/users/u/proj/.lait"),
            Path::new("C:/users/u/AppData/lait/agents"),
            true,
        );
        if cfg!(windows) {
            assert_eq!(scoped.len(), 1, "same dir, different spelling");
        }
        // And a path that merely *starts with the same text* as agents_base is
        // not under it.
        let entries = vec![entry("/home/u/.config/lait/agents-notreally/x")];
        let scoped = scope(
            &entries,
            Path::new("/home/u/.config/lait"),
            Path::new("/home/u/.config/lait/agents"),
            false,
        );
        assert_eq!(scoped.len(), 1, "'agents-notreally' is not under 'agents'");
    }

    #[test]
    fn space_ids_are_stable_and_path_distinct() {
        assert_eq!(space_id("/home/u/a/.lait"), space_id("/home/u/a/.lait"));
        assert_ne!(space_id("/home/u/a/.lait"), space_id("/home/u/b/.lait"));
        assert_eq!(space_id("/home/u/a/.lait").len(), 16);
    }
}
