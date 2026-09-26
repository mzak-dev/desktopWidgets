//! Monitor identity (saved in workspace.json) and the live monitor geometry.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct MonitorRef {
    pub name: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MonitorInfo {
    pub name: String,
    /// Physical px.
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub scale: f64,
    /// Work area (excludes the taskbar), physical px: x, y, w, h.
    pub work: (i32, i32, u32, u32),
}

impl MonitorInfo {
    pub fn reference(&self) -> MonitorRef {
        MonitorRef { name: self.name.clone(), width: self.w, height: self.h }
    }
}
