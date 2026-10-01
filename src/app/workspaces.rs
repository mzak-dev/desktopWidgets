//! Workspaces on screen: switching between them, and the rules that bring one up by itself
//! when the virtual desktop or the connected monitors change (ADR-0011).

use super::*;
use crate::platform::vdesk;
use crate::workspace::MonitorRef;

impl App {
    /// The monitors connected now, as a Workspace's monitor rule records them.
    pub(super) fn monitor_setup(&self) -> Vec<MonitorRef> {
        self.monitors.iter().map(|m| m.reference()).collect()
    }

    /// Reads the virtual desktops again; true when another one is on screen than before.
    pub(super) fn read_desktops(&mut self) -> bool {
        let now = vdesk::current();
        self.desktops = vdesk::desktops();
        let moved = now != self.desktop;
        self.desktop = now;
        if let Some(s) = &mut self.settings {
            s.invalidate();
        }
        moved
    }

    /// Brings up the Workspace whose rules fit best, if one does and it is not up already.
    /// One picked by hand stays until the next desktop or monitor change.
    pub(super) fn follow_rules(&mut self, el: &ActiveEventLoop, why: &str) -> bool {
        match self.ws.pick(self.desktop.as_deref(), &self.monitor_setup()) {
            Some(name) => self.switch_workspace(el, &name, why),
            None => false,
        }
    }

    /// Puts the widgets on screen away and shows Workspace `name`: its own widgets, in its
    /// own look. Undo, a half-confirmed remove and every window belong to the one going away.
    pub(super) fn switch_workspace(&mut self, el: &ActiveEventLoop, name: &str, why: &str) -> bool {
        if !self.ws.switch_to(name) {
            return false;
        }
        self.remove_armed = None;
        self.undo.clear();
        self.wins.clear(); // closes every widget window; `sync_windows` opens the new ones
        self.rebuild_theme();
        self.sync_windows(el);
        self.sync_watchers();
        if let Some(s) = &mut self.settings {
            s.workspace_changed();
        }
        self.refresh_tray();
        self.mark_save();
        self.log(format!("workspace {name}{why}: {} widget(s)", self.ws.instances.len()));
        true
    }

    /// The name of the desktop with this id, as Task View shows it.
    pub(super) fn desktop_name(&self, id: &str) -> String {
        self.desktops.iter().find(|d| d.id.eq_ignore_ascii_case(id)).map_or_else(|| "a removed desktop".into(), |d| d.name.clone())
    }

    pub(super) fn apply_workspace(&mut self, el: &ActiveEventLoop, cmd: WsCmd) {
        match cmd {
            WsCmd::Switch(name) => {
                self.switch_workspace(el, &name, " (picked)");
            }
            WsCmd::Add { copy } => {
                let base = if copy { self.ws.active.clone() } else { "Workspace".to_string() };
                let name = self.ws.add_workspace(&base, copy);
                self.log(format!("added workspace {name}"));
                self.switch_workspace(el, &name, " (new)");
            }
            // sent at every keystroke: an empty or taken name on the way to a good one is no news
            WsCmd::Rename(old, new) => {
                if let Ok(n) = self.ws.rename_workspace(&old, &new) {
                    if n != old {
                        self.refresh_tray();
                        self.mark_save();
                    }
                }
            }
            WsCmd::Delete(name) => {
                // the one on screen goes: show its neighbour first, so the windows follow
                if name == self.ws.active {
                    if let Some(next) = self.ws.neighbour(&name) {
                        self.switch_workspace(el, &next, " (the one shown was removed)");
                    }
                }
                match self.ws.delete_workspace(&name) {
                    Ok(_) => {
                        self.log(format!("removed workspace {name}"));
                        self.refresh_tray();
                        self.mark_save();
                    }
                    Err(e) => self.log(e),
                }
            }
            WsCmd::Desktop(name, desktop, tie) => {
                let label = self.desktop_name(&desktop);
                if let Some(r) = self.ws.rules_mut(&name) {
                    r.desktops.retain(|d| !d.eq_ignore_ascii_case(&desktop));
                    if tie {
                        r.desktops.push(desktop);
                    }
                    self.log(format!("workspace {name} {} {label}", if tie { "follows" } else { "no longer follows" }));
                    self.mark_save();
                }
            }
            WsCmd::Monitors(name, tie) => {
                let setup = self.monitor_setup();
                if let Some(r) = self.ws.rules_mut(&name) {
                    r.monitors = if tie { setup } else { Vec::new() };
                    self.log(format!("workspace {name} {}", if tie { "comes up with these monitors" } else { "no longer follows the monitors" }));
                    self.mark_save();
                }
            }
        }
        if let Some(s) = &mut self.settings {
            s.invalidate();
        }
    }
}
