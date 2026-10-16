use crate::support::{STARTERS, Wayfinder, until};

#[raywright::test]
async fn every_starter_widget_has_a_window_and_shows_elements_to_the_bridge() -> raywright::Result<()> {
    let wf = Wayfinder::launch().await?;
    for id in STARTERS {
        let w = wf.widget(id).await?;
        // what the bridge itself says, so a failure shows why
        let asked = wf.app.bridge().await?.elements(w.win.id().0).await.map(|e| e.len());
        let sources = w.win.elements().await?;
        let from_bridge: usize = sources.iter().filter(|(source, _)| source == "bridge").map(|(_, nodes)| nodes.len()).sum();
        assert!(from_bridge > 0, "{id}: the bridge listed no elements; asked directly: {asked:?}; by source: {:?}", sources.iter().map(|(s, n)| (s.clone(), n.len())).collect::<Vec<_>>());
    }
    let state = wf.state().await?;
    assert_eq!(state["instances"].as_array().map(Vec::len), Some(STARTERS.len()), "{state}");
    wf.close().await
}

#[raywright::test]
async fn clicking_the_folder_opens_it_in_place_and_clicking_again_restores_it() -> raywright::Result<()> {
    let wf = Wayfinder::launch().await?;
    let folder = wf.widget("icon_folder-1").await?;
    let closed = folder.rect().await?;

    folder.action("toggle expanded").click().await?;
    until("the folder grows", 5, || async {
        let r = folder.rect().await?;
        Ok(if r.width > closed.width + 100.0 && r.height > closed.height + 40.0 { Ok(()) } else { Err(format!("{r:?} vs {closed:?}")) })
    })
    .await?;

    folder.action("toggle expanded").click().await?;
    until("the folder is back", 5, || async {
        let r = folder.rect().await?;
        Ok(if r == closed { Ok(()) } else { Err(format!("{r:?} vs {closed:?}")) })
    })
    .await?;
    wf.close().await
}
