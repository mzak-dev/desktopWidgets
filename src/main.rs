#![windows_subsystem = "windows"]
//! Wayfinder: desktop widgets for Windows. See CONTEXT.md and docs/adr/.
//!
//!   wayfinder                     run (tray icon, widgets on the desktop)
//!   wayfinder --data <dir>        use a different data directory
//!   wayfinder --exit-after <sec>  quit by itself (automated runs)
//!   wayfinder --gpu <mode>        high | low | software (this run only)
//!   wayfinder --edit              start in Edit Mode
//!   wayfinder --selftest          drive the interactive paths with synthetic input and report

use wayfinder::app::{App, Options, UserEvent};
use winit::event_loop::EventLoop;

/// Where release builds are published; Velopack reads updates from this repo's GitHub Releases.
const UPDATE_REPO: &str = "https://github.com/mzak-dev/desktopWidgets";

fn arg(name: &str) -> Option<String> {
    let a: Vec<String> = std::env::args().collect();
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1).cloned())
}

/// Checks for a newer release roughly hourly and downloads it, staged for next launch.
/// Never applies or restarts here: `VelopackApp::run()` in `main()` already applies any
/// pending package silently the next time the app starts.
fn spawn_update_checker() {
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(30));
        loop {
            match velopack::UpdateManager::new(velopack::sources::GithubSource::new(UPDATE_REPO, None, false), None, None) {
                Ok(um) => match um.check_for_updates() {
                    Ok(velopack::UpdateCheck::UpdateAvailable(update)) => {
                        if let Err(e) = um.download_updates(&update, None) {
                            eprintln!("wayfinder: update download failed: {e}");
                        }
                    }
                    Ok(_) => {}
                    Err(e) => eprintln!("wayfinder: update check failed: {e}"),
                },
                // Not installed via Velopack (e.g. a dev build) — nothing to check.
                Err(_) => return,
            }
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    });
}

fn main() {
    use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::core::HSTRING;

    // Must run before anything else: on install/update/uninstall Velopack re-invokes this
    // exe with a lifecycle flag, runs the matching hook, and exits — it should never reach
    // the mutex/COM/window setup below on those invocations.
    velopack::VelopackApp::build().run();

    let dir = arg("--data").map(std::path::PathBuf::from).unwrap_or_else(wayfinder::workspace::data_dir);
    // One running copy per data directory: a second launch exits quietly.
    let key = format!("Wayfinder-{:x}", dir.to_string_lossy().bytes().fold(5381u64, |h, b| h.wrapping_mul(33) ^ b as u64));
    let _mutex = unsafe { CreateMutexW(None, true, &HSTRING::from(key)) };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        eprintln!("wayfinder: already running (data dir {})", dir.display());
        return;
    }

    // COM for the file pickers; S_FALSE (already initialised) is fine.
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
    }
    let event_loop = EventLoop::<UserEvent>::with_user_event().build().expect("event loop");
    let proxy = event_loop.create_proxy();
    let exit_after_secs = arg("--exit-after").and_then(|s| s.parse().ok());
    let selftest = std::env::args().any(|a| a == "--selftest");
    let mut app = App::new(proxy, Options { dir, exit_after_secs, selftest, gpu_override: arg("--gpu") });
    if std::env::args().any(|a| a == "--edit") {
        app.request_edit_on_start();
    }
    // Skip on automated runs (selftest, --exit-after): they don't live long enough to matter
    // and shouldn't make network calls.
    if !selftest && exit_after_secs.is_none() {
        spawn_update_checker();
    }
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("wayfinder: event loop error: {e}");
    }
}
