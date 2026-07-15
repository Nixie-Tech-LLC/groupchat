//! crossterm events → semantic [`Action`]s. Input-consuming layers (editor;
//! later picker/palette) eat raw keys before the keymap; everything else
//! resolves through the per-context binding tables. Mouse: Stage-1 basics
//! (click to focus/select, wheel to scroll) over the render-time hit regions —
//! the full model (double-click, tabs interactions) lands in Stage 4.

use anyhow::Result;
use crossterm::event::{KeyEvent, KeyEventKind, MouseEvent, MouseEventKind};

use super::action::Action;
use super::app::{App, HitTarget, OverlayLayer};
use super::keymap::FocusKind;
use super::panels::help;

pub async fn dispatch_key(app: &mut App, ev: KeyEvent) -> Result<()> {
    if ev.kind != KeyEventKind::Press {
        return Ok(());
    }
    // Editor layer consumes raw input.
    if matches!(app.stack.last(), Some(OverlayLayer::Editor(_))) {
        if let Some((intent, content)) = app.handle_editor_key(ev) {
            app.submit_editor(intent, content).await?;
        }
        return Ok(());
    }
    let ctx = app.focus();
    let Some(action) = app.keymap.resolve(ctx, &ev) else {
        return Ok(());
    };
    // The help overlay's Enter runs the highlighted action in the underlying
    // context (actionable help): pop, then apply.
    if ctx == FocusKind::Help && action == Action::Submit {
        let rows = help::entries(app, underlying_ctx(app));
        let sel = app.help_sel.min(rows.len().saturating_sub(1));
        if let Some((_, _, chosen)) = rows.get(sel) {
            let chosen = *chosen;
            app.stack.pop();
            app.help_sel = 0;
            return Box::pin(app.apply(chosen)).await;
        }
        return Ok(());
    }
    app.apply(action).await
}

/// The context the help overlay describes (what's under it).
pub fn underlying_ctx(app: &App) -> FocusKind {
    match (app.screen, &app.peek) {
        (super::app::Screen::Board, Some(p)) if p.focused => FocusKind::Peek,
        (super::app::Screen::Board, _) => FocusKind::Board,
        _ => FocusKind::List,
    }
}

pub async fn dispatch_mouse(app: &mut App, ev: MouseEvent) -> Result<()> {
    match ev.kind {
        MouseEventKind::Down(_) => {
            let target = app
                .regions
                .iter()
                .rev()
                .find(|r| contains(r.rect, ev.column, ev.row))
                .map(|r| r.target);
            match target {
                Some(HitTarget::ProjectTab(i)) => {
                    if i != app.project_idx && i < app.projects.len() {
                        app.project_idx = i;
                        app.peek = None;
                        app.reload_board().await?;
                    }
                }
                Some(HitTarget::ColumnHeader(c)) => {
                    app.col_idx = c;
                    app.clamp_selection();
                }
                Some(HitTarget::BoardRow { col, row }) => {
                    app.col_idx = col;
                    app.row_idx = row;
                    app.clamp_selection();
                    if let Some(p) = &mut app.peek {
                        p.focused = false;
                    }
                }
                Some(HitTarget::Peek) => {
                    if let Some(p) = &mut app.peek {
                        p.focused = true;
                    }
                }
                Some(HitTarget::LegendAction(a)) => Box::pin(app.apply(a)).await?,
                Some(HitTarget::ListRow(i)) => {
                    if matches!(app.stack.last(), Some(OverlayLayer::Help)) {
                        app.help_sel = i;
                    } else {
                        let s = app.screen;
                        app.list_cursors.entry(s).or_default().sel = i;
                    }
                }
                None => {}
            }
        }
        MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
            let down = ev.kind == MouseEventKind::ScrollDown;
            // Scroll the panel UNDER the cursor: peek if hit, else the board.
            let over_peek = app
                .regions
                .iter()
                .rev()
                .find(|r| contains(r.rect, ev.column, ev.row))
                .is_some_and(|r| r.target == HitTarget::Peek);
            if over_peek {
                if let Some(p) = &mut app.peek {
                    p.scroll = if down {
                        p.scroll.saturating_add(2)
                    } else {
                        p.scroll.saturating_sub(2)
                    };
                }
            } else {
                app.apply(if down { Action::Down } else { Action::Up })
                    .await?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn contains(rect: ratatui::layout::Rect, x: u16, y: u16) -> bool {
    x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
}
