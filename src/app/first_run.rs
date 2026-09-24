use super::*;

/// How-tos for editing the data folder (themes, plugins) with Claude Code, as path parts under it.
const GUIDES: &[(&[&str], &str)] = &[
    (&["THEMES.md"], include_str!("../../assets/guides/THEMES.md")),
    (&[".claude", "skills", "wayfinder-theme", "SKILL.md"], include_str!("../../assets/guides/wayfinder-theme/SKILL.md")),
    (&["PLUGINS.md"], include_str!("../../assets/guides/PLUGINS.md")),
    (&[".claude", "skills", "wayfinder-plugin", "SKILL.md"], include_str!("../../assets/guides/wayfinder-plugin/SKILL.md")),
];

/// Written only when missing, so the user's own edits survive every start.
pub(super) fn write_missing_guides(dir: &std::path::Path) -> Vec<String> {
    let mut errors = Vec::new();
    for (parts, text) in GUIDES {
        let path = parts.iter().fold(dir.to_path_buf(), |p, part| p.join(part));
        if path.exists() {
            continue;
        }
        let written = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::write(&path, text));
        if let Err(e) = written {
            errors.push(format!("could not write {}: {e}", path.display()));
        }
    }
    errors
}

/// Right-hand side of the primary monitor, clear of the desktop icons.
pub(super) fn default_instances(monitors: &[MonitorInfo], reg: &Registry, card: Card, host: &mut dyn Host) -> Vec<InstanceCfg> {
    let Some(m) = monitors.iter().find(|m| m.x == 0 && m.y == 0).or(monitors.first()) else { return vec![] };
    let logical_w = m.work.2 as f32 / m.scale as f32;
    let size = |id: &str| match reg.get(id) {
        Some(Ok(d)) => card.window_size(d.meta().default_card_size),
        _ => (240.0, 160.0),
    };
    let mut out = Vec::new();
    let mut col = |id: &str, widget: &str, x_from_right: f32, y: f32| {
        let (w, h) = size(widget);
        let mut c = InstanceCfg { id: id.into(), widget: widget.into(), monitor: m.reference(), x: (logical_w - x_from_right - w).max(0.0), y, w, h, ..Default::default() };
        if let Some(Ok(wd)) = reg.get(widget) {
            widgets::set_up_instance(&**wd, &mut c, host);
        }
        out.push(c);
        (w, h)
    };
    let (cw, ch) = col("clock-1", "clock", 8.0, 8.0);
    let (_, dh) = col("digital_clock-1", "digital_clock", 8.0, 8.0 + ch - 20.0);
    let x2 = 8.0 + cw.max(340.0) - 20.0;
    let (lw, lh) = col("icon_list-1", "icon_list", x2, 8.0);
    col("icon_folder-1", "icon_folder", x2 + lw - 20.0, 8.0);
    let _ = (dh, lh);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guides_are_written_once_and_user_edits_survive() {
        let dir = std::env::temp_dir().join(format!("wf-guides-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(write_missing_guides(&dir).is_empty());
        let skill = dir.join(".claude").join("skills").join("wayfinder-theme").join("SKILL.md");
        assert_eq!(std::fs::read_to_string(&skill).unwrap(), GUIDES[1].1);
        std::fs::write(dir.join("THEMES.md"), "mine").unwrap();
        assert!(write_missing_guides(&dir).is_empty());
        assert_eq!(std::fs::read_to_string(dir.join("THEMES.md")).unwrap(), "mine");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_plugins_guide_example_manifest_is_valid() {
        // a Windows checkout may give the guide CRLF line endings
        let guide = GUIDES.iter().find(|(p, _)| p == &["PLUGINS.md"]).unwrap().1.replace("\r\n", "\n");
        let example = guide.split("```toml\n").nth(1).and_then(|b| b.split("```").next()).expect("a toml example");
        let m = crate::plugins::Manifest::parse(example).expect("the example parses");
        assert_eq!(m.id, "sunset");
        let code = guide.split("```toml\n").nth(2).and_then(|b| b.split("```").next()).expect("a [code] example");
        let with_code = crate::plugins::Manifest::parse(&format!("{example}\n{code}")).expect("the [code] example parses");
        assert_eq!(with_code.code.iter().map(|c| c.source.as_str()).collect::<Vec<_>>(), ["weather"]);
    }

    #[test]
    fn themes_guide_names_every_builtin_token() {
        let guide = GUIDES[0].1;
        let lib = Library::load(std::path::Path::new("no-such-dir"));
        for axis in [&lib.palettes[0], &lib.fonts[0], &lib.glyphs[0]] {
            for token in axis.tokens.keys() {
                assert!(guide.contains(token.as_str()), "THEMES.md does not mention `{token}`");
            }
        }
    }
}
