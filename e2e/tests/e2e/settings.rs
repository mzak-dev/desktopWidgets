use raywright::prelude::*;

use crate::support::Wayfinder;

#[raywright::test]
async fn settings_opens_on_a_page_and_switches_pages_by_its_tabs() -> raywright::Result<()> {
    let wf = Wayfinder::launch().await?;
    assert!(wf.state().await?["settings"].is_null(), "the tests start from widgets alone");

    // widgets on top would cover the Settings window's tabs and take the click
    wf.call("set_z", serde_json::json!({ "mode": "desktop" })).await?;
    let settings = wf.open_settings("widgets").await?;
    expect(settings.tab("plugins")).to_be_visible().await?;
    assert_eq!(wf.state().await?["settings"], "widgets");

    settings.tab("plugins").click().await?;
    crate::support::until("the Plugins page shows", 5, || async {
        let page = wf.state().await?["settings"].clone();
        Ok(if page == "plugins" { Ok(()) } else { Err(format!("on {page}")) })
    })
    .await?;

    wf.call("close_settings", serde_json::json!({})).await?;
    expect(wf.app.clone()).not().to_have_window("title=\"Wayfinder Settings\"").await?;
    wf.close().await
}
