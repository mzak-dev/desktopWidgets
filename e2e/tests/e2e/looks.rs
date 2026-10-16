use raywright::prelude::*;

use crate::support::Wayfinder;

/// What a widget draws, as the widget itself draws it: the pictures stored under
/// `tests/__screenshots__` are made the same way on every machine (the software renderer,
/// the app's own frame, nothing in front of it). Widgets that show the time are not here.
#[raywright::test]
async fn the_folder_and_the_list_look_as_they_did() -> raywright::Result<()> {
    let wf = Wayfinder::launch().await?;
    for id in ["icon_folder-1", "icon_list-1"] {
        let w = wf.widget(id).await?;
        expect(w.win.clone()).to_have_screenshot(&format!("{id}.png")).source(CaptureSource::App).await?;
    }
    wf.close().await
}
