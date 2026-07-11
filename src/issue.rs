//! Layer A — the Issue document (SCHEMA §5). One Loro document per issue,
//! addressed by [`DocId`]. This wrapper owns the container layout and exposes
//! typed reads/writes; **all merge semantics live in Loro** (S§1). A "register"
//! is a single key in the root `LoroMap` resolved by Lamport order (LWW).
//!
//! Fields (S§5):
//! - `id`, `workspaceId`, `projectId`, `createdBy`, `createdAt` — value leaves.
//! - `title`, `status`, `priority` — LWW value leaves.
//! - `description` — `LoroText` (RGA char-merge, co-editable).
//! - `assignees`, `labels` — `LoroMap<Id, true>` present-key sets (S§5.2).
//! - `comments` — `LoroList<Comment>`, insertion-order union (S§5.3).
//!
//! `projectId` is the **single source of project membership** (S§5.5).

use anyhow::{anyhow, Result};
use loro::{ExportMode, Frontiers, LoroDoc};

use crate::dto::{CommentDto, Priority, DEFAULT_STATUS};
use crate::ids::{DocId, LabelId, ProjectId, UserId, WorkspaceId};
use crate::loro_ext as lx;

const ROOT: &str = "issue";
const K_ID: &str = "id";
const K_WORKSPACE: &str = "workspaceId";
const K_PROJECT: &str = "projectId";
const K_TITLE: &str = "title";
const K_STATUS: &str = "status";
const K_PRIORITY: &str = "priority";
const K_CREATED_BY: &str = "createdBy";
const K_CREATED_AT: &str = "createdAt";
const C_DESCRIPTION: &str = "description";
const C_ASSIGNEES: &str = "assignees";
const C_LABELS: &str = "labels";
const C_COMMENTS: &str = "comments";

/// Parameters for creating a fresh issue.
pub struct NewIssue {
    pub doc_id: DocId,
    pub workspace_id: WorkspaceId,
    pub project_id: ProjectId,
    pub title: String,
    pub priority: Priority,
    pub created_by: UserId,
    pub created_at: u64,
    pub body: Option<String>,
}

/// A wrapper around one issue's `LoroDoc`.
pub struct IssueDoc {
    doc: LoroDoc,
}

impl IssueDoc {
    /// Create a brand-new issue document, committing the initial state.
    pub fn create(spec: NewIssue) -> Result<Self> {
        let doc = LoroDoc::new();
        let root = doc.get_map(ROOT);
        root.insert(K_ID, spec.doc_id.as_str())?;
        root.insert(K_WORKSPACE, spec.workspace_id.as_str())?;
        root.insert(K_PROJECT, spec.project_id.as_str())?;
        root.insert(K_TITLE, spec.title.as_str())?;
        root.insert(K_STATUS, DEFAULT_STATUS)?;
        root.insert(K_PRIORITY, spec.priority.as_str())?;
        root.insert(K_CREATED_BY, spec.created_by.as_str())?;
        root.insert(K_CREATED_AT, spec.created_at as i64)?;
        // create the description text container (empty or seeded body)
        let desc = root.insert_container(C_DESCRIPTION, loro::LoroText::new())?;
        if let Some(body) = spec.body {
            if !body.is_empty() {
                desc.insert(0, &body)?;
            }
        }
        root.insert_container(C_ASSIGNEES, loro::LoroMap::new())?;
        root.insert_container(C_LABELS, loro::LoroMap::new())?;
        root.insert_container(C_COMMENTS, loro::LoroList::new())?;
        doc.commit();
        Ok(Self { doc })
    }

    /// Wrap an already-loaded `LoroDoc` (from the store or from sync).
    pub fn from_doc(doc: LoroDoc) -> Self {
        Self { doc }
    }

    /// Borrow the underlying document (for the store / sync layer).
    pub fn doc(&self) -> &LoroDoc {
        &self.doc
    }

    /// Export a full snapshot (durable store / cold-start sync).
    pub fn snapshot(&self) -> Result<Vec<u8>> {
        self.doc
            .export(ExportMode::Snapshot)
            .map_err(|e| anyhow!("export issue snapshot: {e}"))
    }

    /// Import bytes (a snapshot or an update) into this doc.
    pub fn import(&self, bytes: &[u8]) -> Result<()> {
        self.doc
            .import(bytes)
            .map(|_| ())
            .map_err(|e| anyhow!("import issue update: {e}"))
    }

    /// The issue doc's oplog frontiers — the causal head used as the sync digest
    /// (SCHEMA §3.2, §8).
    pub fn head(&self) -> Frontiers {
        self.doc.oplog_frontiers()
    }

    fn root(&self) -> loro::LoroMap {
        self.doc.get_map(ROOT)
    }

    /// The `description` `LoroText`, nested under the root `issue` map.
    fn description_text(&self) -> Option<loro::LoroText> {
        match self.root().get(C_DESCRIPTION) {
            Some(loro::ValueOrContainer::Container(loro::Container::Text(t))) => Some(t),
            _ => None,
        }
    }

    /// The `comments` `LoroList`, nested under the root `issue` map.
    fn comments_list(&self) -> Option<loro::LoroList> {
        match self.root().get(C_COMMENTS) {
            Some(loro::ValueOrContainer::Container(loro::Container::List(l))) => Some(l),
            _ => None,
        }
    }

    // ---- reads ----

    pub fn doc_id(&self) -> Option<DocId> {
        lx::get_str(&self.root(), K_ID).and_then(|s| DocId::parse(&s))
    }
    pub fn workspace_id(&self) -> Option<WorkspaceId> {
        lx::get_str(&self.root(), K_WORKSPACE).and_then(|s| WorkspaceId::parse(&s))
    }
    pub fn project_id(&self) -> Option<ProjectId> {
        lx::get_str(&self.root(), K_PROJECT).and_then(|s| ProjectId::parse(&s))
    }
    pub fn title(&self) -> String {
        lx::get_str(&self.root(), K_TITLE).unwrap_or_default()
    }
    pub fn status(&self) -> String {
        lx::get_str(&self.root(), K_STATUS).unwrap_or_else(|| DEFAULT_STATUS.to_string())
    }
    pub fn priority(&self) -> Priority {
        lx::get_str(&self.root(), K_PRIORITY)
            .and_then(|s| Priority::parse(&s))
            .unwrap_or_default()
    }
    pub fn created_by(&self) -> Option<UserId> {
        lx::get_str(&self.root(), K_CREATED_BY).map(UserId::from_key_string)
    }
    pub fn created_at(&self) -> u64 {
        lx::get_u64(&self.root(), K_CREATED_AT).unwrap_or(0)
    }
    pub fn description(&self) -> String {
        self.description_text()
            .map(|t| t.to_string())
            .unwrap_or_default()
    }

    /// Assignee keys present in the set (S§5.2).
    pub fn assignees(&self) -> Vec<UserId> {
        let mut out: Vec<UserId> = lx::get_map(&self.root(), C_ASSIGNEES)
            .map(|m| lx::present_keys(&m))
            .unwrap_or_default()
            .into_iter()
            .map(UserId::from_key_string)
            .collect();
        out.sort();
        out
    }

    /// Label ids present in the set (S§5.2).
    pub fn labels(&self) -> Vec<LabelId> {
        let mut out: Vec<LabelId> = lx::get_map(&self.root(), C_LABELS)
            .map(|m| lx::present_keys(&m))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|s| LabelId::parse(&s))
            .collect();
        out.sort();
        out
    }

    /// Comments in insertion order (S§5.3).
    pub fn comments(&self) -> Vec<CommentDto> {
        let Some(list) = self.comments_list() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for i in 0..list.len() {
            let Some(v) = list.get(i) else { continue };
            let m = match v {
                loro::ValueOrContainer::Container(loro::Container::Map(m)) => m,
                _ => continue,
            };
            out.push(CommentDto {
                author: UserId::from_key_string(lx::get_str(&m, "author").unwrap_or_default()),
                author_nick: None,
                ts: lx::get_u64(&m, "ts").unwrap_or(0),
                body: lx::get_str(&m, "body").unwrap_or_default(),
            });
        }
        out
    }

    // ---- writes (each caller commits at the Request boundary, S§7.1) ----

    pub fn set_title(&self, title: &str) -> Result<()> {
        self.root().insert(K_TITLE, title)?;
        Ok(())
    }
    pub fn set_status(&self, status: &str) -> Result<()> {
        self.root().insert(K_STATUS, status)?;
        Ok(())
    }
    pub fn set_priority(&self, priority: Priority) -> Result<()> {
        self.root().insert(K_PRIORITY, priority.as_str())?;
        Ok(())
    }
    pub fn set_project(&self, project: &ProjectId) -> Result<()> {
        self.root().insert(K_PROJECT, project.as_str())?;
        Ok(())
    }

    /// Replace the whole description (P0 full-buffer replace, UI.md §5.3).
    pub fn set_description(&self, body: &str) -> Result<()> {
        let t = self
            .description_text()
            .ok_or_else(|| anyhow!("description container missing"))?;
        let len = t.len_unicode();
        if len > 0 {
            t.delete(0, len)?;
        }
        if !body.is_empty() {
            t.insert(0, body)?;
        }
        Ok(())
    }

    pub fn add_assignee(&self, user: &UserId) -> Result<()> {
        lx::get_map(&self.root(), C_ASSIGNEES)
            .ok_or_else(|| anyhow!("assignees container missing"))?
            .insert(user.as_str(), true)?;
        Ok(())
    }
    pub fn remove_assignee(&self, user: &UserId) -> Result<()> {
        if let Some(m) = lx::get_map(&self.root(), C_ASSIGNEES) {
            if m.get(user.as_str()).is_some() {
                m.delete(user.as_str())?;
            }
        }
        Ok(())
    }
    pub fn add_label(&self, label: &LabelId) -> Result<()> {
        lx::get_map(&self.root(), C_LABELS)
            .ok_or_else(|| anyhow!("labels container missing"))?
            .insert(label.as_str(), true)?;
        Ok(())
    }
    pub fn remove_label(&self, label: &LabelId) -> Result<()> {
        if let Some(m) = lx::get_map(&self.root(), C_LABELS) {
            if m.get(label.as_str()).is_some() {
                m.delete(label.as_str())?;
            }
        }
        Ok(())
    }

    /// Append an immutable comment (S§5.3).
    pub fn add_comment(&self, author: &UserId, ts: u64, body: &str) -> Result<()> {
        let list = self
            .comments_list()
            .ok_or_else(|| anyhow!("comments container missing"))?;
        let map = list.insert_container(list.len(), loro::LoroMap::new())?;
        map.insert("author", author.as_str())?;
        map.insert("ts", ts as i64)?;
        map.insert("body", body)?;
        Ok(())
    }

    /// Commit the pending ops as one Loro commit (one Request = one commit, S§7.1).
    pub fn commit(&self) {
        self.doc.commit();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::SystemUlidSource;

    fn ws() -> WorkspaceId {
        WorkspaceId::mint(&SystemUlidSource)
    }
    fn prj() -> ProjectId {
        ProjectId::mint(&SystemUlidSource)
    }
    fn doc() -> DocId {
        DocId::mint(&SystemUlidSource)
    }
    fn user() -> UserId {
        UserId::from_key_string("a".repeat(64))
    }

    fn sample() -> IssueDoc {
        IssueDoc::create(NewIssue {
            doc_id: doc(),
            workspace_id: ws(),
            project_id: prj(),
            title: "fix login".into(),
            priority: Priority::High,
            created_by: user(),
            created_at: 1000,
            body: Some("the token refresh races".into()),
        })
        .unwrap()
    }

    #[test]
    fn create_and_read_back() {
        let i = sample();
        assert_eq!(i.title(), "fix login");
        assert_eq!(i.status(), "backlog");
        assert_eq!(i.priority(), Priority::High);
        assert_eq!(i.description(), "the token refresh races");
        assert_eq!(i.created_at(), 1000);
        assert!(i.doc_id().is_some());
        assert!(i.assignees().is_empty());
    }

    #[test]
    fn edit_lww_fields() {
        let i = sample();
        i.set_title("fix login race").unwrap();
        i.set_status("in_progress").unwrap();
        i.set_priority(Priority::Urgent).unwrap();
        i.commit();
        assert_eq!(i.title(), "fix login race");
        assert_eq!(i.status(), "in_progress");
        assert_eq!(i.priority(), Priority::Urgent);
    }

    #[test]
    fn assignees_are_a_present_key_set() {
        let i = sample();
        let u1 = UserId::from_key_string("a".repeat(64));
        let u2 = UserId::from_key_string("b".repeat(64));
        i.add_assignee(&u1).unwrap();
        i.add_assignee(&u2).unwrap();
        i.commit();
        assert_eq!(i.assignees().len(), 2);
        i.remove_assignee(&u1).unwrap();
        i.commit();
        assert_eq!(i.assignees(), vec![u2]);
    }

    #[test]
    fn labels_add_remove() {
        let i = sample();
        let l = LabelId::mint(&SystemUlidSource);
        i.add_label(&l).unwrap();
        i.commit();
        assert_eq!(i.labels(), vec![l.clone()]);
        i.remove_label(&l).unwrap();
        i.commit();
        assert!(i.labels().is_empty());
    }

    #[test]
    fn comments_append_immutably() {
        let i = sample();
        i.add_comment(&user(), 10, "first").unwrap();
        i.add_comment(&user(), 20, "second").unwrap();
        i.commit();
        let cs = i.comments();
        assert_eq!(cs.len(), 2);
        assert_eq!(cs[0].body, "first");
        assert_eq!(cs[1].ts, 20);
    }

    #[test]
    fn description_full_replace() {
        let i = sample();
        i.set_description("brand new body").unwrap();
        i.commit();
        assert_eq!(i.description(), "brand new body");
    }

    #[test]
    fn snapshot_roundtrip_preserves_state() {
        let i = sample();
        i.set_status("done").unwrap();
        i.commit();
        let snap = i.snapshot().unwrap();
        let loaded = IssueDoc::from_doc({
            let d = LoroDoc::new();
            d.import(&snap).unwrap();
            d
        });
        assert_eq!(loaded.title(), "fix login");
        assert_eq!(loaded.status(), "done");
    }
}
