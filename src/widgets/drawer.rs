//! A Rust Widget drawn from `drawer.toml`, so a user copy of that file restyles it.

use std::path::Path;
use std::sync::Arc;

use super::{ActionCx, Built, Host, Inputs, TomlWidget, Widget, WidgetMeta};
use crate::theme::Theme;
use crate::value::Value;
use crate::workspace::InstanceCfg;

pub struct Drawer {
    tree: TomlWidget,
}

impl Drawer {
    pub fn wrap(tree: TomlWidget) -> Arc<dyn Widget> {
        Arc::new(Drawer { tree })
    }
}

impl Widget for Drawer {
    fn meta(&self) -> &WidgetMeta {
        self.tree.meta()
    }

    fn build(&self, inp: &Inputs, theme: &Theme, image_size: &dyn Fn(&str) -> Option<(f32, f32)>) -> Result<Built, String> {
        self.tree.build(inp, theme, image_size)
    }

    fn on_instance_added(&self, cfg: &mut InstanceCfg, host: &mut dyn Host) {
        let dir = host.data_dir().join("drawers").join(&cfg.id);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            host.log(format!("could not create {}: {e}", dir.display()));
        }
        cfg.set_param("folder", &Value::Str(dir.to_string_lossy().into_owned()));
    }

    fn handle_action(&self, verb: &str, _arg: &str, cx: &mut ActionCx) -> bool {
        if verb != "add_app" {
            return false;
        }
        let Some(picked) = cx.host.pick_file() else { return true };
        let folder = cx.cfg.folder();
        if folder.is_empty() {
            cx.host.log("this drawer has no shortcut folder".into());
            return true;
        }
        if !cx.host.create_shortcut(Path::new(&folder), &picked) {
            cx.host.log(format!("could not create a shortcut to `{}`", picked.display()));
        }
        cx.sources.invalidate(); // the folder watcher would also catch it, a moment later
        cx.wants_redraw = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::DataSources;
    use crate::widgets::{Registry, set_up_instance};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    #[derive(Default)]
    struct FakeHost {
        dir: PathBuf,
        pick: Option<PathBuf>,
        shortcuts: Vec<(PathBuf, PathBuf)>,
        logs: Vec<String>,
    }

    impl Host for FakeHost {
        fn data_dir(&self) -> &Path {
            &self.dir
        }
        fn pick_file(&mut self) -> Option<PathBuf> {
            self.pick.clone()
        }
        fn create_shortcut(&mut self, dir: &Path, target: &Path) -> bool {
            self.shortcuts.push((dir.into(), target.into()));
            true
        }
        fn log(&mut self, msg: String) {
            self.logs.push(msg);
        }
    }

    fn drawer(user_dir: &Path) -> Arc<dyn Widget> {
        match Registry::load(user_dir).get("drawer") {
            Some(Ok(w)) => w.clone(),
            other => panic!("drawer: {:?}", other.map(|d| d.as_ref().err())),
        }
    }

    #[test]
    fn a_new_drawer_gets_a_folder_of_its_own() {
        let dir = std::env::temp_dir().join(format!("wf-drawer-{}", std::process::id()));
        let mut host = FakeHost { dir: dir.clone(), ..Default::default() };
        let mut cfg = InstanceCfg { id: "drawer-3".into(), widget: "drawer".into(), ..Default::default() };
        set_up_instance(&*drawer(Path::new("nope")), &mut cfg, &mut host);
        let want = dir.join("drawers").join("drawer-3");
        assert_eq!(cfg.folder(), want.to_string_lossy());
        assert!(want.is_dir());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn add_app_puts_a_shortcut_in_the_folder_and_other_verbs_pass_through() {
        let w = drawer(Path::new("nope"));
        let mut cfg = InstanceCfg::default();
        cfg.set_param("folder", &Value::Str("D:\\drawer".into()));
        let mut host = FakeHost { pick: Some(PathBuf::from("C:\\Apps\\app.exe")), ..Default::default() };
        let (mut state, sources) = (BTreeMap::new(), DataSources::builtin());
        let mut cx = ActionCx { cfg: &cfg, state: &mut state, host: &mut host, sources: &sources, wants_redraw: false };
        assert!(w.handle_action("add_app", "", &mut cx));
        assert!(cx.wants_redraw);
        assert!(!w.handle_action("toggle", "collapsed", &mut cx), "engine verbs are not the drawer's");
        assert_eq!(host.shortcuts, [(PathBuf::from("D:\\drawer"), PathBuf::from("C:\\Apps\\app.exe"))]);
    }

    #[test]
    fn a_user_copy_of_drawer_toml_restyles_it_and_keeps_its_behaviour() {
        let dir = std::env::temp_dir().join(format!("wf-drawer-user-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("drawer.toml"), "name = 'My Drawer'\n[params.folder]\ntype='path'\n[root]\ntype='box'").unwrap();
        let w = drawer(&dir);
        assert_eq!(w.meta().name, "My Drawer");
        let mut host = FakeHost { dir: dir.clone(), ..Default::default() };
        let mut cfg = InstanceCfg { id: "drawer-1".into(), ..Default::default() };
        set_up_instance(&*w, &mut cfg, &mut host);
        assert!(!cfg.folder().is_empty(), "still a Rust Widget underneath");
        std::fs::remove_dir_all(&dir).ok();
    }
}
