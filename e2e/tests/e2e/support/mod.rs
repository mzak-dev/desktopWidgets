//! How to start Wayfinder and what its windows are called: the profile every test shares.

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use raywright::prelude::*;
use raywright::{AppProfile, Config};
use serde_json::{Value, json};

pub mod pages;

pub use pages::{SettingsWindow, WidgetWindow};

/// The widgets a fresh data folder starts with.
pub const STARTERS: [&str; 4] = ["clock-1", "digital_clock-1", "icon_list-1", "icon_folder-1"];

fn exe() -> PathBuf {
    match std::env::var_os("WAYFINDER_EXE") {
        Some(p) => PathBuf::from(p),
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/debug/wayfinder.exe"),
    }
}

/// Wayfinder with a data folder of its own, the bridge on and the software renderer: the
/// same pixels on any machine.
pub struct WayfinderProfile;

#[async_trait]
impl AppProfile for WayfinderProfile {
    fn name(&self) -> &str {
        "wayfinder"
    }

    fn launch_spec(&self, _config: &Config) -> raywright::Result<LaunchSpec> {
        let exe = exe();
        assert!(exe.exists(), "build Wayfinder first: cargo build --features automation ({} is missing)", exe.display());
        Ok(LaunchSpec::exe(exe).args(["--automation", "--gpu", "software"]).temp_data_dir("--data").wait_for_bridge().wait_for_windows(STARTERS.len()))
    }

    /// A first run opens Settings for onboarding; the tests start from widgets alone. And
    /// widgets sit on the desktop, under every window: other programs (a CI runner's agent)
    /// would cover them, so they go on top for the test.
    async fn ready(&self, app: &App) -> raywright::Result<()> {
        let bridge = app.bridge().await?;
        bridge.call("set_z", json!({ "mode": "topmost" })).await?;
        if bridge.call("state", json!({})).await?["settings"].is_string() {
            bridge.call("close_settings", json!({})).await?;
            for _ in 0..50 {
                if bridge.call("state", json!({})).await?["settings"].is_null() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        Ok(())
    }
}

/// A running Wayfinder.
pub struct Wayfinder {
    pub rw: Raywright,
    pub app: App,
}

impl Wayfinder {
    pub async fn launch() -> raywright::Result<Wayfinder> {
        let rw = Raywright::new()?;
        let app = rw.launch_profile(&WayfinderProfile).await?;
        Ok(Wayfinder { rw, app })
    }

    pub async fn widget(&self, id: &str) -> raywright::Result<WidgetWindow> {
        Ok(WidgetWindow { win: self.app.wait_for_window(&format!("title=\"Wayfinder {id}\""), Duration::from_secs(10)).await? })
    }

    /// Opens Settings (on `page`) and returns it.
    pub async fn open_settings(&self, page: &str) -> raywright::Result<SettingsWindow> {
        self.call("open_settings", json!({ "page": page })).await?;
        Ok(SettingsWindow { win: self.app.wait_for_window("title=\"Wayfinder Settings\"", Duration::from_secs(10)).await? })
    }

    /// One of Wayfinder's own commands (`tests` list them in `hello`).
    pub async fn call(&self, name: &str, args: Value) -> raywright::Result<Value> {
        self.app.bridge().await?.call(name, args).await
    }

    pub async fn state(&self) -> raywright::Result<Value> {
        self.call("state", json!({})).await
    }

    pub async fn close(self) -> raywright::Result<()> {
        self.app.close().await.map(drop)
    }
}

/// Polls `f` for up to `secs` seconds until it holds; the last answer is the failure message.
pub async fn until<F, Fut>(what: &str, secs: u64, mut f: F) -> raywright::Result<()>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = raywright::Result<Result<(), String>>>,
{
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    loop {
        match f().await? {
            Ok(()) => return Ok(()),
            Err(why) if std::time::Instant::now() >= deadline => panic!("{what}: {why}"),
            Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
}
