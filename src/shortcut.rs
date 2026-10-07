//! A launcher entry (Shortcut) and its icon-id helpers, shared by data, workspace and icons.

use std::path::Path;

use crate::value::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct Shortcut {
    pub name: String,
    pub target: String,
    /// Overrides the target's own icon when not empty.
    pub icon: String,
}

impl Shortcut {
    pub fn from_value(v: &Value) -> Option<Shortcut> {
        let s = |k: &str| v.get(k).map(|x| x.to_string()).unwrap_or_default();
        let target = s("target");
        if target.is_empty() {
            return None;
        }
        let name = if s("name").is_empty() { file_stem(&target) } else { s("name") };
        Some(Shortcut { name, target, icon: s("icon") })
    }

    pub fn to_value(&self) -> Value {
        Value::obj([("name", self.name.as_str().into()), ("target", self.target.as_str().into()), ("icon", self.icon.as_str().into())])
    }
}

pub fn file_stem(target: &str) -> String {
    let t = target.trim_end_matches(['\\', '/']);
    Path::new(t).file_stem().and_then(|s| s.to_str()).unwrap_or(t).to_string()
}

/// Not a legal path character.
pub const ID_SEP: char = '\u{1f}';

pub fn icon_id(pack: &str, s: &Shortcut) -> String {
    format!("icon:{pack}{ID_SEP}{}{ID_SEP}{}", s.target, s.icon)
}
