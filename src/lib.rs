//! Wayfinder: a desktop widget engine. See CONTEXT.md and docs/adr/.

pub mod anim;
pub mod app;
pub mod card;
pub mod cli;
pub mod code;
pub mod color;
pub mod content;
pub mod data;
pub mod dialog;
pub mod draw;
pub mod edit;
pub mod elements;
pub mod expr;
pub mod format;
pub mod gfx;
pub mod icons;
pub mod images;
pub mod meta;
pub mod modules;
pub mod monitor;
pub mod net;
pub mod platform;
pub mod plugins;
pub mod settings;
pub mod shortcut;
pub mod suggest;
pub mod text;
pub mod theme;
pub mod thumbs;
pub mod ui;
pub mod value;
pub mod widgets;
pub mod workspace;

#[cfg(test)]
mod module_graph;

pub use app::{Options, run};
pub use data::{DataSource, Notifier, SourceCx};
