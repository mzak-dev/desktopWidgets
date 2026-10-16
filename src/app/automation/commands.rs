//! What a test or an agent can ask Wayfinder to do through `call`. One line per command:
//! add a command here and it shows up in the bridge's `hello`.

use raywright_bridge::CommandRegistry;
use serde_json::{Value, json};

use super::{App, Then};
use crate::settings::Cmd;

fn text(args: &Value, name: &str) -> Option<String> {
    args.get(name).and_then(Value::as_str).map(String::from)
}

pub(super) fn registry() -> CommandRegistry<App> {
    let mut r = CommandRegistry::<App>::new();
    r.add("state", "edit mode, the active workspace, every instance, the open settings page", |app, _| Ok(app.automation_state()));
    r.add("set_edit", "{ on: bool }: turn Edit Mode on or off", |app, a| {
        app.set_edit(a.get("on").and_then(Value::as_bool).unwrap_or(true));
        Ok(json!({ "edit": app.edit }))
    });
    r.add("open_settings", "{ page?: string }: open Settings, on a page", |app, a| {
        app.then(Then::Settings(text(&a, "page")));
        Ok(Value::Null)
    });
    r.add("close_settings", "close the Settings window", |app, _| {
        app.then(Then::Cmd(Cmd::Close));
        Ok(Value::Null)
    });
    r.add("finish_setup", "end the first-run setup, as its Finish button does (Settings then shows its pages)", |app, _| {
        app.then(Then::Cmd(Cmd::Onboarded));
        Ok(Value::Null)
    });
    r.add("set_z", "{ id?: string, mode: string }: where a widget sits (desktop, bottom, normal, topmost); every widget without an id", |app, a| {
        let mode = text(&a, "mode").ok_or("set_z needs a mode")?;
        let ids: Vec<String> = match text(&a, "id") {
            Some(id) => vec![id],
            None => app.ws.instances.iter().map(|c| c.id.clone()).collect(),
        };
        for id in ids {
            app.then(Then::Cmd(Cmd::Z(id, mode.clone())));
        }
        Ok(Value::Null)
    });
    r.add("menu", "{ id: string }: pick a tray menu entry (edit, settings, reload, folder, wsmanage, ws:N)", |app, a| {
        let id = text(&a, "id").ok_or("menu needs an id")?;
        app.then(Then::Menu(id));
        Ok(Value::Null)
    });
    r.add("reload", "read widget definitions, themes and folders again", |app, _| {
        app.then(Then::Menu("reload".into()));
        Ok(Value::Null)
    });
    r.add("quit", "exit Wayfinder", |app, _| {
        app.then(Then::Menu("quit".into()));
        Ok(Value::Null)
    });
    r
}

impl App {
    fn then(&mut self, t: Then) {
        if let Some(a) = &mut self.automation {
            a.then.push(t);
        }
    }
}
