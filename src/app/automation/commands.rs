//! What a test or an agent can ask Wayfinder to do through `call`. One line per command:
//! add a command here and it shows up in the bridge's `hello`.

use raywright_bridge::CommandRegistry;
use serde_json::{Value, json};

use super::{App, Then};

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
