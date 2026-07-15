//! Body panels — plain modules with `fn draw(f, app, rect, …)`, dispatched by
//! `Screen` in `super::draw_body`. No trait objects; this is a client, not a
//! framework.

pub mod activity;
pub mod board;
pub mod doctor;
pub mod help;
pub mod inbox;
pub mod members;
pub mod peek;
pub mod spaces;
