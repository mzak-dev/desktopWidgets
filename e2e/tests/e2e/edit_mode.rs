use raywright::prelude::*;

use crate::support::{Wayfinder, until};

#[raywright::test]
async fn edit_mode_turns_on_and_off() -> raywright::Result<()> {
    let wf = Wayfinder::launch().await?;
    assert_eq!(wf.state().await?["edit"], false);
    wf.call("set_edit", serde_json::json!({ "on": true })).await?;
    assert_eq!(wf.state().await?["edit"], true);
    wf.call("set_edit", serde_json::json!({ "on": false })).await?;
    assert_eq!(wf.state().await?["edit"], false);
    wf.close().await
}

#[raywright::test]
async fn dragging_a_widget_in_edit_mode_moves_it_and_saves_the_place() -> raywright::Result<()> {
    let wf = Wayfinder::launch().await?;
    let clock = wf.widget("clock-1").await?;
    wf.call("set_edit", serde_json::json!({ "on": true })).await?;
    until("edit mode reaches the window", 5, || async { Ok(if wf.state().await?["edit"] == true { Ok(()) } else { Err("not yet".into()) }) }).await?;

    let before = clock.rect().await?;
    let from = before.center();
    clock.win.mouse().drag(from, Point::new(from.x - 120.0, from.y + 80.0), 12).await?;

    until("the window moved", 5, || async {
        let r = clock.rect().await?;
        let (dx, dy) = (r.x - before.x, r.y - before.y);
        Ok(if (dx + 120.0).abs() <= 12.0 && (dy - 80.0).abs() <= 12.0 { Ok(()) } else { Err(format!("moved by ({dx}, {dy})")) })
    })
    .await?;

    // saved a moment after the drag ends, anchored to a monitor
    let file = wf.app.data_dir().expect("a data folder of its own").join("workspace.json");
    let moved = wf.state().await?["instances"].as_array().and_then(|a| a.iter().find(|i| i["id"] == "clock-1").cloned()).expect("clock-1");
    until("workspace.json has the new place", 5, || async {
        let saved: Option<serde_json::Value> = std::fs::read_to_string(&file).ok().and_then(|t| serde_json::from_str(&t).ok());
        let at = saved.as_ref().and_then(|w| w["instances"].as_array()).and_then(|a| a.iter().find(|i| i["id"] == "clock-1")).map(|i| (i["x"].clone(), i["y"].clone()));
        Ok(if at == Some((moved["x"].clone(), moved["y"].clone())) { Ok(()) } else { Err(format!("{at:?} vs ({}, {})", moved["x"], moved["y"])) })
    })
    .await?;
    wf.close().await
}
