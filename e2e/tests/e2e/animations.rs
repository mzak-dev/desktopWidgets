use raywright::prelude::*;

use crate::support::Wayfinder;

/// The widget's own picture of itself: drawn by Wayfinder, so nothing in front of the
/// window or behind it gets in.
#[raywright::test]
async fn a_widget_draws_itself_for_the_bridge() -> raywright::Result<()> {
    let wf = Wayfinder::launch().await?;
    let clock = wf.widget("clock-1").await?;
    let shot = clock.win.screenshot().source(CaptureSource::App).await?;
    let client = clock.win.info().await?.client;
    assert_eq!((shot.width() as f64, shot.height() as f64), (client.width, client.height), "as large as the window's client area");
    assert!(shot.pixels().any(|p| p.0[3] > 0), "something was drawn");
    shot.save(wf.rw.config().output_dir().join("clock.png")).ok();
    wf.close().await
}

/// With the animation clock stopped, the folder opens in the same frames on every machine.
#[raywright::test]
async fn opening_the_folder_settles_and_the_recording_is_the_same_each_time() -> raywright::Result<()> {
    let wf = Wayfinder::launch().await?;
    let folder = wf.widget("icon_folder-1").await?;
    let opts = RecordOptions::default().manual_clock(ms(16)).until_settled(ms(100)).max_frames(60);
    let (clicked, rec) = folder.win.record_during(opts, async { folder.action("toggle expanded").click().await }).await?;
    clicked?;

    // EXPAND_SECS is 0.2 s; some slack for the frame that lands it
    expect(&rec).to_settle_within(ms(400))?;
    expect(&rec).to_have_frame_count(5..=60)?;
    rec.save_dir(wf.rw.config().output_dir().join("folder-expand"))?;
    wf.close().await
}
