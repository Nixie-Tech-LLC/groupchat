//! Body panels — plain modules with `fn draw(f, app, rect, …)`, dispatched by
//! `Screen` in `super::draw_body`. No trait objects; this is a client, not a
//! framework.

pub mod board;
pub mod doctor;
pub mod help;
pub mod peek;
