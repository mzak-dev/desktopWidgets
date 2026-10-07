//! Every built-in Widget lays out cleanly at every size it allows: its default and a 4x4 grid
//! from its min to its max, so a size tier cannot break in between. The rules are the layout
//! flags of `scene::flags` and the cases are the `fits` scene set (`scenes/fits/`), the same
//! check `wayfinder scene check fits` runs; the tests below read what a Widget builds from the
//! Scene Dump.

use std::path::Path;

use super::Registry;
use crate::scene;

#[test]
fn every_builtin_fits_every_size_it_allows() {
    let report = scene::check_set("fits");
    assert!(report.is_clean(), "
{}", report.failures().join("
"));
    // a scene file per Widget, each with the default size and the grid, with and without every
    // switch on: a Widget added without one, or a file cut down, is caught here
    let reg = Registry::load(Path::new("no-such-dir"));
    for id in reg.ids() {
        let cases = report.rows.iter().filter(|r| r.id.starts_with(&format!("fits/{id}@"))).count();
        assert_eq!(cases, 17 * 2, "scenes/fits/{id}.scene.toml: the default size and a 4x4 grid, each with two variants");
    }
}

/// Every text a Widget builds at `size` with its defaults, the way the app reads its sources.
fn texts_at(id: &str, size: (f32, f32)) -> Vec<String> {
    let dump = scene::dump_widget(id, size).unwrap_or_else(|e| panic!("{id}: {e}"));
    dump.texts().into_iter().map(|(_, t)| t.to_string()).collect()
}

#[test]
fn a_big_clock_shows_its_default_cities_and_a_small_one_does_not() {
    let tall = texts_at("clock", (300.0, 360.0));
    assert!(tall.iter().any(|t| t == "London") && !tall.iter().any(|t| t == "Tokyo"), "tall: as many chips as fit under the face (two at 300 px): {tall:?}");
    assert!(texts_at("clock", (520.0, 220.0)).iter().any(|t| t == "London"), "wide: a list beside it");
    assert!(!texts_at("clock", (220.0, 220.0)).iter().any(|t| t == "Tokyo"), "default size: just the face");
}

#[test]
fn a_tall_digital_clock_adds_city_chips() {
    assert!(texts_at("digital_clock", (340.0, 220.0)).iter().any(|t| t == "London"));
    assert!(!texts_at("digital_clock", (300.0, 132.0)).iter().any(|t| t == "London"), "default size: time and date only");
}

#[test]
fn the_monitor_trades_detail_for_room() {
    let compact = texts_at("system_monitor", (200.0, 90.0));
    assert!(compact.iter().any(|t| t == "CPU") && !compact.iter().any(|t| t.starts_with("Up ")), "compact: bars, no footer: {compact:?}");
    let normal = texts_at("system_monitor", (340.0, 190.0));
    assert!(normal.iter().any(|t| t.starts_with("Up ")) && !normal.iter().any(|t| t == "Commit" || t == "Download"), "normal: gauges and uptime: {normal:?}");
    let large = texts_at("system_monitor", (620.0, 380.0));
    assert!(["Commit", "Download", "Memory"].iter().all(|w| large.iter().any(|t| t == w)), "large: more gauges and history: {large:?}");
}

#[test]
fn the_icon_list_goes_from_icons_to_rows_to_a_grid() {
    let names = |w: f32| texts_at("icon_list", (w, 320.0)).into_iter().filter(|t| t == "Notepad").count();
    assert_eq!((names(150.0), names(260.0), names(420.0)), (0, 1, 1), "narrow: icons only; then a named row; then a named tile");
}
