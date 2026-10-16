use raywright::prelude::*;

use crate::support::Wayfinder;

#[raywright::test]
async fn settings_opens_on_a_page_and_switches_pages_by_its_tabs() -> raywright::Result<()> {
    let wf = Wayfinder::launch().await?;
    assert!(wf.state().await?["settings"].is_null(), "the tests start from widgets alone");

    let settings = wf.open_settings("widgets").await?;
    expect(settings.tab("plugins")).to_be_visible().await?;
    assert_eq!(wf.state().await?["settings"], "widgets");

    // Real input first; if the page does not change, try messages posted to the window and
    // say so, so a failure shows which kind of input the Settings window ignores.
    let on_plugins = || async { Ok::<_, raywright::Error>(wf.state().await?["settings"] == "plugins") };
    settings.tab("plugins").click().await?;
    for _ in 0..30 {
        if on_plugins().await? {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    if !on_plugins().await? {
        settings.tab("plugins").click().mode(InputMode::Message).await?;
        for _ in 0..30 {
            if on_plugins().await? {
                panic!("OS input did not switch the page but posted messages did");
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        panic!("neither OS input nor posted messages switched the page: on {}", wf.state().await?["settings"]);
    }

    wf.call("close_settings", serde_json::json!({})).await?;
    expect(wf.app.clone()).not().to_have_window("title=\"Wayfinder Settings\"").await?;
    wf.close().await
}
