use raywright::prelude::*;

use crate::support::Wayfinder;

#[raywright::test]
async fn settings_opens_on_a_page_and_switches_pages_by_its_tabs() -> raywright::Result<()> {
    let wf = Wayfinder::launch().await?;
    assert!(wf.state().await?["settings"].is_null(), "the tests start from widgets alone");

    let settings = wf.open_settings("widgets").await?;
    expect(settings.tab("plugins")).to_be_visible().await?;
    assert_eq!(wf.state().await?["settings"], "widgets");

    // Messages posted to the window, not OS input: on GitHub's runner a real click on the
    // Settings window does not switch the page while the same click as messages does (found
    // by trying both). The widgets take OS input fine; why Settings does not is open.
    settings.tab("plugins").click().mode(InputMode::Message).await?;
    crate::support::until("the Plugins page shows", 5, || async {
        let page = wf.state().await?["settings"].clone();
        Ok(if page == "plugins" { Ok(()) } else { Err(format!("on {page}")) })
    })
    .await?;

    wf.call("close_settings", serde_json::json!({})).await?;
    expect(wf.app.clone()).not().to_have_window("title=\"Wayfinder Settings\"").await?;
    wf.close().await
}
