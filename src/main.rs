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

fn arg(name: &str) -> Option<String> {
    let a: Vec<String> = std::env::args().collect();
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1).cloned())
}

fn main() {
    use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::core::HSTRING;

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
    let mut app = App::new(proxy, Options { dir, exit_after_secs, selftest: std::env::args().any(|a| a == "--selftest"), gpu_override: arg("--gpu") });
    if std::env::args().any(|a| a == "--edit") {
        app.request_edit_on_start();
    }
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("wayfinder: event loop error: {e}");
    }
}
