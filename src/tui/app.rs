//! App state + the daemon-facing side of the TUI: reload fns, doorbell
//! routing (U§4.2 — every dirty scope refreshes exactly the panels that show
//! it), the optimistic [`Overlay`] (U§4.3, moved verbatim from the old
//! client), action execution, and the focus model.
//!
//! Focus is DERIVED, never stored: the top of the overlay `stack` wins, then
//! peek-vs-board, then the active screen. `Esc` pops stack → closes peek →
//! returns to Board → quits.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;
use ratatui::layout::Rect;

use crate::control::{request, CatalogScope, Doorbell, Request, Response};
use crate::diagnose::DiagnosisView;
use crate::dto::{ActivityEvent, BoardView, IssueView, ProjectDto, Row};

use super::action::Action;
use super::keymap::{FocusKind, Keymap};
use super::theme::Theme;
use super::widgets::editor::{EditorIntent, EditorOutcome, EditorState};
use super::widgets::statusbar::StatusLine;

/// Full-body screens. Board is the root; the rest land across stages (an
/// unbuilt screen renders a stub so navigation never dead-ends).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Screen {
    Board,
    Inbox,
    Activity,
    Members,
    Spaces,
    ConfigPanel,
    Doctor,
    Remotes,
    Log,
}

/// The right-side issue detail, co-visible with the board (NOT on the overlay
/// stack — a picker can sit over peek over board and all three render).
pub struct PeekState {
    pub view: IssueView,
    pub scroll: u16,
    pub expanded: bool,
    pub focused: bool,
}

/// Modal layers; top of the stack owns input.
pub enum OverlayLayer {
    Editor(Box<EditorState>),
    Help,
}

/// Mouse hit-testing: regions are rebuilt every draw (base first, overlays
/// last; lookup scans backwards so the top layer eats the click).
pub struct HitRegion {
    pub rect: Rect,
    pub target: HitTarget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitTarget {
    ProjectTab(usize),
    ColumnHeader(usize),
    BoardRow { col: usize, row: usize },
    Peek,
    ListRow(usize),
    LegendAction(Action),
}

/// The optimistic overlay: a local prediction keyed by `(doc_id, field)`,
/// cleared on any doorbell for its scope (U§4.3). Correlation-free.
#[derive(Debug, Default)]
pub struct Overlay {
    by_doc: HashMap<String, HashMap<String, String>>,
}

impl Overlay {
    pub fn set(&mut self, doc_id: &str, field: &str, value: &str) {
        self.by_doc
            .entry(doc_id.to_string())
            .or_default()
            .insert(field.to_string(), value.to_string());
    }
    pub fn clear_doc(&mut self, doc_id: &str) {
        self.by_doc.remove(doc_id);
    }
    pub fn get<'a>(&'a self, doc_id: &str, field: &str) -> Option<&'a str> {
        self.by_doc
            .get(doc_id)
            .and_then(|m| m.get(field))
            .map(|s| s.as_str())
    }
    pub fn has(&self, doc_id: &str) -> bool {
        self.by_doc.contains_key(doc_id)
    }
}

/// Per-list cursor + scroll window for list-shaped screens.
#[derive(Debug, Default, Clone, Copy)]
pub struct ListCursor {
    pub sel: usize,
    #[allow(dead_code)] // list windowing lands with the Stage-2 list_picker
    pub scroll: usize,
}

pub struct App {
    pub home: PathBuf,
    // ---- daemon-derived data (plain DTOs; tests construct these directly) ----
    pub projects: Vec<ProjectDto>,
    pub project_idx: usize,
    applied_default_project: bool,
    pub board: Option<BoardView>,
    #[allow(dead_code)] // the Activity screen renders these in Stage 3
    pub activity: Vec<ActivityEvent>,
    pub inbox_unread: u64,
    pub diagnosis: Option<DiagnosisView>,
    pub peers_online: usize,
    // ---- prediction ----
    pub overlay: Overlay,
    // ---- UI state ----
    pub screen: Screen,
    pub peek: Option<PeekState>,
    pub stack: Vec<OverlayLayer>,
    pub col_idx: usize,
    pub row_idx: usize,
    pub list_cursors: HashMap<Screen, ListCursor>,
    pub filter_text: String,
    /// Cursor into the actionable `?` help overlay.
    pub help_sel: usize,
    pub theme: Theme,
    pub keymap: Keymap,
    pub regions: Vec<HitRegion>,
    pub status: StatusLine,
    pub quit: bool,
}

impl App {
    pub fn new(home: PathBuf, theme: Theme, keymap: Keymap) -> Self {
        App {
            home,
            projects: Vec::new(),
            project_idx: 0,
            applied_default_project: false,
            board: None,
            activity: Vec::new(),
            inbox_unread: 0,
            diagnosis: None,
            peers_online: 0,
            overlay: Overlay::default(),
            screen: Screen::Board,
            peek: None,
            stack: Vec::new(),
            col_idx: 0,
            row_idx: 0,
            list_cursors: HashMap::new(),
            filter_text: String::new(),
            help_sel: 0,
            theme,
            keymap,
            regions: Vec::new(),
            status: StatusLine::default(),
            quit: false,
        }
    }

    // ---- focus ----

    /// The input-owning context. Editor/Help layers consume raw keys before
    /// the keymap; the returned kind picks the binding table otherwise.
    pub fn focus(&self) -> FocusKind {
        if matches!(self.stack.last(), Some(OverlayLayer::Help)) {
            return FocusKind::Help;
        }
        match (self.screen, &self.peek) {
            (Screen::Board, Some(p)) if p.focused => FocusKind::Peek,
            (Screen::Board, _) => FocusKind::Board,
            _ => FocusKind::List,
        }
    }

    pub fn editor_mut(&mut self) -> Option<&mut EditorState> {
        match self.stack.last_mut() {
            Some(OverlayLayer::Editor(e)) => Some(e.as_mut()),
            _ => None,
        }
    }

    fn push_editor(&mut self, ed: EditorState) {
        self.stack.push(OverlayLayer::Editor(Box::new(ed)));
    }

    // ---- data access ----

    pub fn current_project(&self) -> Option<&ProjectDto> {
        self.projects.get(self.project_idx)
    }

    /// Rows of the focused board column, post-filter.
    pub fn column_rows(&self, col: usize) -> Vec<&Row> {
        let Some(b) = &self.board else {
            return Vec::new();
        };
        let Some(c) = b.columns.get(col) else {
            return Vec::new();
        };
        c.rows
            .iter()
            .filter(|r| self.row_matches_filter(r))
            .collect()
    }

    pub fn row_matches_filter(&self, r: &Row) -> bool {
        if self.filter_text.is_empty() {
            return true;
        }
        let needle = self.filter_text.to_lowercase();
        r.title.to_lowercase().contains(&needle)
            || r.reff.to_lowercase().contains(&needle)
            || r.key_alias
                .as_deref()
                .is_some_and(|a| a.to_lowercase().contains(&needle))
    }

    pub fn focused_row(&self) -> Option<Row> {
        self.column_rows(self.col_idx)
            .get(self.row_idx)
            .map(|r| (*r).clone())
    }

    /// The overlay-aware field reads (U§4.3): prediction wins until a doorbell
    /// clears it.
    pub fn effective_title(&self, r: &Row) -> String {
        self.overlay
            .get(r.doc_id.as_str(), "title")
            .map(str::to_string)
            .unwrap_or_else(|| r.title.clone())
    }
    pub fn effective_status(&self, r: &Row) -> String {
        self.overlay
            .get(r.doc_id.as_str(), "status")
            .map(str::to_string)
            .unwrap_or_else(|| r.status.clone())
    }

    pub fn clamp_selection(&mut self) {
        let ncols = self.board.as_ref().map(|b| b.columns.len()).unwrap_or(0);
        if ncols == 0 {
            self.col_idx = 0;
            self.row_idx = 0;
            return;
        }
        self.col_idx = self.col_idx.min(ncols - 1);
        let nrows = self.column_rows(self.col_idx).len();
        self.row_idx = if nrows == 0 {
            0
        } else {
            self.row_idx.min(nrows - 1)
        };
    }

    // ---- daemon round-trips ----

    pub async fn req(&self, req: Request) -> Result<Response> {
        request(&self.home, &req).await
    }

    pub async fn reload_projects(&mut self) -> Result<()> {
        if let Response::Projects { projects } = self.req(Request::ProjectList).await? {
            self.projects = projects;
            if self.project_idx >= self.projects.len() {
                self.project_idx = 0;
            }
            if !self.applied_default_project {
                self.applied_default_project = true;
                if let Some(dflt) =
                    crate::config::Settings::load(Some(&self.home)).default_project()
                {
                    if let Some(i) = self
                        .projects
                        .iter()
                        .position(|p| p.key.eq_ignore_ascii_case(&dflt))
                    {
                        self.project_idx = i;
                    }
                }
            }
        }
        Ok(())
    }

    pub async fn reload_board(&mut self) -> Result<()> {
        let Some(p) = self.current_project().map(|p| p.key.clone()) else {
            self.board = None;
            return Ok(());
        };
        match self
            .req(Request::Board {
                project: Some(p),
                project_hint: None,
            })
            .await?
        {
            Response::Board(b) => {
                self.board = Some(*b);
                self.clamp_selection();
            }
            Response::Error { message, .. } => self.status.error(message),
            _ => {}
        }
        Ok(())
    }

    /// Re-fetch the peek's issue (its doc went dirty, or an edit landed).
    pub async fn refresh_peek(&mut self) -> Result<()> {
        let Some(reff) = self.peek.as_ref().map(|p| p.view.reff.clone()) else {
            return Ok(());
        };
        if let Response::Issue(v) = self.req(Request::IssueView { reff }).await? {
            if let Some(p) = &mut self.peek {
                p.view = *v;
            }
        }
        Ok(())
    }

    pub async fn refresh_inbox_count(&mut self) {
        if let Ok(Response::Inbox { unread, .. }) = self.req(Request::Inbox { clear: false }).await
        {
            self.inbox_unread = unread;
        }
    }

    pub async fn reload_diagnosis(&mut self) -> Result<()> {
        if let Response::Diagnosis(v) = self
            .req(Request::Diagnose {
                expected_workspace: None,
            })
            .await?
        {
            self.diagnosis = Some(*v);
        }
        Ok(())
    }

    pub async fn refresh_status_info(&mut self) {
        if let Ok(Response::Status(s)) = self.req(Request::Status).await {
            self.peers_online = s.online_peers;
        }
    }

    /// Refresh whatever the current screen shows (used on doorbell + `r`).
    pub async fn refresh_current(&mut self) -> Result<()> {
        match self.screen {
            Screen::Board => self.reload_board().await?,
            Screen::Doctor => self.reload_diagnosis().await?,
            // Later-stage screens refresh once they hold data.
            _ => {}
        }
        Ok(())
    }

    /// Doorbell routing (U§4.2): every dirty scope refreshes exactly the
    /// panels that render it. Doorbells are dirty-notices — re-read, never
    /// patch.
    pub async fn on_doorbell(&mut self, db: Doorbell) -> Result<()> {
        if db.reset {
            self.overlay = Overlay::default();
            self.reload_projects().await?;
            self.refresh_current().await?;
            self.refresh_peek().await?;
            self.refresh_inbox_count().await;
            return Ok(());
        }
        let current_project = self
            .current_project()
            .map(|p| p.id.as_str().to_string())
            .unwrap_or_default();
        let mut board_dirty = false;
        let mut peek_dirty = false;
        let peek_doc = self
            .peek
            .as_ref()
            .map(|p| p.view.doc_id.as_str().to_string());
        for (proj, docs) in &db.dirty_by_project {
            for d in docs {
                self.overlay.clear_doc(d);
                if Some(d.as_str()) == peek_doc.as_deref() {
                    peek_dirty = true;
                }
            }
            if *proj == current_project {
                board_dirty = true;
            }
        }
        for scope in &db.dirty_catalog {
            match scope {
                CatalogScope::Projects => self.reload_projects().await?,
                CatalogScope::Workflow => board_dirty = true,
                CatalogScope::Boards { project } if *project == current_project => {
                    board_dirty = true
                }
                _ => {}
            }
        }
        if board_dirty && self.screen == Screen::Board {
            self.reload_board().await?;
        }
        if peek_dirty {
            self.refresh_peek().await?;
        }
        if db.activity_advanced {
            self.refresh_inbox_count().await;
        }
        if db.presence_advanced {
            self.refresh_status_info().await;
        }
        Ok(())
    }

    // ---- action execution ----

    pub async fn apply(&mut self, action: Action) -> Result<()> {
        use Action::*;
        match action {
            Quit => self.quit = true,
            Back => {
                if self.stack.pop().is_some() {
                } else if self.peek.is_some() {
                    self.peek = None;
                } else if self.screen != Screen::Board {
                    self.screen = Screen::Board;
                } else {
                    self.quit = true;
                }
            }
            Help => self.stack.push(OverlayLayer::Help),
            Refresh => {
                self.refresh_current().await?;
                self.refresh_peek().await?;
            }
            Goto(s) => {
                self.screen = s;
                self.refresh_current().await?;
            }
            NextProject | PrevProject => {
                if !self.projects.is_empty() {
                    let n = self.projects.len();
                    self.project_idx = if action == NextProject {
                        (self.project_idx + 1) % n
                    } else {
                        (self.project_idx + n - 1) % n
                    };
                    self.peek = None;
                    self.reload_board().await?;
                }
            }
            Up | Down | Left | Right | Top | Bottom => self.motion(action),
            OpenPeek => self.open_peek().await?,
            TogglePeekFocus => {
                if let Some(p) = &mut self.peek {
                    p.focused = !p.focused;
                }
            }
            ExpandPeek => {
                if let Some(p) = &mut self.peek {
                    p.expanded = !p.expanded;
                    p.focused = true;
                }
            }
            StatusPrev | StatusNext => self.status_move(action == StatusNext).await?,
            Create => self.push_editor(EditorState::new(
                EditorIntent::Create,
                "new issue   (title words, then -p KEY -P prio -l label -a who)",
                "",
            )),
            EditTitle => {
                if let Some(t) = self.target_reff() {
                    let initial = self.target_title();
                    self.push_editor(EditorState::new(
                        EditorIntent::EditTitle { reff: t },
                        "edit title",
                        &initial,
                    ));
                }
            }
            EditDescription => {
                if let Some(t) = self.target_reff() {
                    // Description lives on the full IssueView — the peek holds it.
                    let initial = self
                        .peek
                        .as_ref()
                        .map(|p| p.view.description.clone())
                        .unwrap_or_default();
                    self.push_editor(EditorState::new(
                        EditorIntent::EditDescription { reff: t },
                        "edit description",
                        &initial,
                    ));
                }
            }
            Comment => {
                if let Some(t) = self.target_reff() {
                    self.push_editor(EditorState::new(
                        EditorIntent::Comment { reff: t },
                        "comment",
                        "",
                    ));
                }
            }
            StartIssue | DoneIssue | StopIssue => {
                if let Some(reff) = self.target_reff() {
                    let req = match action {
                        StartIssue => Request::IssueStart { reff },
                        DoneIssue => Request::IssueDone { reff },
                        _ => Request::IssueStop { reff },
                    };
                    match self.req(req).await? {
                        Response::Issue(v) => {
                            self.status.info(format!(
                                "{}  {}",
                                v.key_alias.as_deref().unwrap_or(&v.reff),
                                v.status
                            ));
                            if let Some(p) = &mut self.peek {
                                if p.view.doc_id == v.doc_id {
                                    p.view = *v;
                                }
                            }
                            self.reload_board().await?;
                        }
                        Response::Error { message, .. } => self.status.error(message),
                        _ => {}
                    }
                }
            }
            YankRef => {
                if let Some(reff) = self.target_reff() {
                    if crate::cli::copy_to_clipboard(&reff) {
                        self.status.info(format!("yanked {reff}"));
                    } else {
                        self.status.error("clipboard unavailable");
                    }
                }
            }
            Submit => {
                // Help overlay's "run action" lands in mod.rs (needs the
                // highlighted row); other Submits are handled by their layers.
            }
            // Stage 2+ actions — visible in help, honest about arrival.
            OpenPalette | OpenFilter | PickAssign | PickLabel | PickPriority | PickStatus
            | PickMoveProject | ReorderUp | ReorderDown | Delete | ToggleSelect
            | ClearSelection | InboxClear | SpaceSwitch | PinFilterAsTab | TabNext | TabPrev
            | Cancel => {
                self.status.info(format!(
                    "'{}' lands later in this branch (stage 2+)",
                    action.id()
                ));
            }
        }
        Ok(())
    }

    fn motion(&mut self, action: Action) {
        use Action::*;
        // Help overlay on top: j/k move its selection.
        if matches!(self.stack.last(), Some(OverlayLayer::Help)) {
            match action {
                Down => self.help_sel = self.help_sel.saturating_add(1),
                Up => self.help_sel = self.help_sel.saturating_sub(1),
                Top => self.help_sel = 0,
                _ => {}
            }
            return;
        }
        // Peek-focused: j/k scroll the detail.
        if let Some(p) = &mut self.peek {
            if p.focused {
                match action {
                    Down => p.scroll = p.scroll.saturating_add(1),
                    Up => p.scroll = p.scroll.saturating_sub(1),
                    Top => p.scroll = 0,
                    _ => {}
                }
                return;
            }
        }
        match self.screen {
            Screen::Board => {
                let ncols = self.board.as_ref().map(|b| b.columns.len()).unwrap_or(0);
                match action {
                    Down => self.row_idx = self.row_idx.saturating_add(1),
                    Up => self.row_idx = self.row_idx.saturating_sub(1),
                    Right if ncols > 0 => {
                        self.col_idx = (self.col_idx + 1).min(ncols - 1);
                    }
                    Left => self.col_idx = self.col_idx.saturating_sub(1),
                    Top => self.row_idx = 0,
                    Bottom => self.row_idx = usize::MAX, // clamped below
                    _ => {}
                }
                self.clamp_selection();
            }
            s => {
                let cur = self.list_cursors.entry(s).or_default();
                match action {
                    Down => cur.sel = cur.sel.saturating_add(1),
                    Up => cur.sel = cur.sel.saturating_sub(1),
                    Top => cur.sel = 0,
                    Bottom => cur.sel = usize::MAX,
                    _ => {}
                }
            }
        }
    }

    async fn open_peek(&mut self) -> Result<()> {
        let Some(row) = self.focused_row() else {
            return Ok(());
        };
        match self
            .req(Request::IssueView {
                reff: row.reff.clone(),
            })
            .await?
        {
            Response::Issue(v) => {
                self.peek = Some(PeekState {
                    view: *v,
                    scroll: 0,
                    expanded: false,
                    focused: false,
                });
            }
            Response::Error { message, .. } => self.status.error(message),
            _ => {}
        }
        Ok(())
    }

    /// The issue an action targets: the focused peek's issue when peek has
    /// focus, else the focused board row.
    pub fn target_reff(&self) -> Option<String> {
        if let Some(p) = &self.peek {
            if p.focused || self.screen != Screen::Board {
                return Some(p.view.reff.clone());
            }
        }
        self.focused_row().map(|r| r.reff)
    }

    fn target_title(&self) -> String {
        if let Some(p) = &self.peek {
            if p.focused {
                return p.view.title.clone();
            }
        }
        self.focused_row()
            .map(|r| self.effective_title(&r))
            .unwrap_or_default()
    }

    /// Move the focused issue to the prev/next workflow column (H/L):
    /// optimistic status overlay + `IssueEdit`, rolled back on error.
    async fn status_move(&mut self, next: bool) -> Result<()> {
        let Some(b) = &self.board else {
            return Ok(());
        };
        let states: Vec<String> = b.columns.iter().map(|c| c.state.id.clone()).collect();
        let Some(row) = self.focused_row() else {
            return Ok(());
        };
        let cur_status = self.effective_status(&row);
        let Some(pos) = states.iter().position(|s| *s == cur_status) else {
            return Ok(());
        };
        let target = if next {
            if pos + 1 >= states.len() {
                return Ok(());
            }
            states[pos + 1].clone()
        } else {
            if pos == 0 {
                return Ok(());
            }
            states[pos - 1].clone()
        };
        self.overlay.set(row.doc_id.as_str(), "status", &target);
        let resp = self
            .req(Request::IssueEdit {
                reff: row.reff.clone(),
                title: None,
                status: Some(target),
                priority: None,
                description: None,
            })
            .await?;
        if let Response::Error { message, .. } = resp {
            self.overlay.clear_doc(row.doc_id.as_str());
            self.status.error(message);
        }
        Ok(())
    }

    /// Submit an editor layer's content (the intent→Request mapping carried
    /// over from the old modal, plus quick-create through the CLI grammar).
    pub async fn submit_editor(&mut self, intent: EditorIntent, content: String) -> Result<()> {
        let content_trimmed = content.trim();
        let req = match &intent {
            EditorIntent::Create => {
                if content_trimmed.is_empty() {
                    return Ok(());
                }
                // The quick-create line IS `new`'s grammar (U§6): tokenize and
                // parse through cmdspec so -p/-a/-P/-l/-b mean exactly what
                // they mean in a shell.
                let mut argv: Vec<String> = vec!["lait".to_string(), "new".to_string()];
                let tokens = super::palette::tokenize(content_trimmed);
                // Bare leading words are the title; collect until the first
                // flag token, then pass the rest through.
                let mut title_words = Vec::new();
                let mut rest = Vec::new();
                let mut in_flags = false;
                for t in tokens {
                    if t.starts_with('-') {
                        in_flags = true;
                    }
                    if in_flags {
                        rest.push(t);
                    } else {
                        title_words.push(t);
                    }
                }
                argv.push(title_words.join(" "));
                argv.extend(rest);
                let argv_ref: Vec<&str> = argv.iter().map(String::as_str).collect();
                match crate::cmdspec::parse_to_dispatch(&argv_ref) {
                    Ok(crate::cmdspec::ParsedCommand::Request(r)) => r,
                    Ok(_) => return Ok(()),
                    Err(e) => {
                        // Reopen the editor with the content intact and the
                        // clap error inline — a typo must never eat the line.
                        let mut ed = EditorState::new(
                            EditorIntent::Create,
                            "new issue   (title words, then -p KEY -P prio -l label -a who)",
                            content_trimmed,
                        );
                        ed.error = Some(e.to_string().lines().next().unwrap_or("").to_string());
                        self.push_editor(ed);
                        return Ok(());
                    }
                }
            }
            EditorIntent::EditTitle { reff } => {
                if content_trimmed.is_empty() {
                    return Ok(());
                }
                if let Some(row) = self.focused_row() {
                    self.overlay
                        .set(row.doc_id.as_str(), "title", content_trimmed);
                }
                Request::IssueEdit {
                    reff: reff.clone(),
                    title: Some(content_trimmed.to_string()),
                    status: None,
                    priority: None,
                    description: None,
                }
            }
            EditorIntent::EditDescription { reff } => Request::IssueEdit {
                reff: reff.clone(),
                title: None,
                status: None,
                priority: None,
                description: Some(content.trim_end().to_string()),
            },
            EditorIntent::Comment { reff } => {
                if content_trimmed.is_empty() {
                    return Ok(());
                }
                Request::Comment {
                    reff: reff.clone(),
                    body: content.trim_end().to_string(),
                }
            }
            EditorIntent::NameTab => return Ok(()), // Stage 4
        };
        match self.req(req).await? {
            Response::Ref { reff } => {
                self.status.info(reff);
                self.reload_board().await?;
                self.refresh_peek().await?;
            }
            Response::Error { message, .. } => {
                self.status.error(message);
                // Roll back any optimistic prediction for the edited doc.
                if let Some(row) = self.focused_row() {
                    self.overlay.clear_doc(row.doc_id.as_str());
                }
            }
            _ => {
                self.reload_board().await?;
                self.refresh_peek().await?;
            }
        }
        Ok(())
    }

    /// Feed a key to the top editor layer. Returns `Some((intent, content))`
    /// on submit (the layer is popped); `None` while typing or on cancel.
    pub fn handle_editor_key(
        &mut self,
        ev: crossterm::event::KeyEvent,
    ) -> Option<(EditorIntent, String)> {
        let outcome = match self.stack.last_mut() {
            Some(OverlayLayer::Editor(e)) => e.handle_key(ev),
            _ => return None,
        };
        match outcome {
            EditorOutcome::Consumed => None,
            EditorOutcome::Cancel => {
                self.stack.pop();
                None
            }
            EditorOutcome::Submit(content) => match self.stack.pop() {
                Some(OverlayLayer::Editor(e)) => Some((e.intent, content)),
                _ => None,
            },
        }
    }
}
