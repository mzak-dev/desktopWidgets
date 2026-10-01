//! Windows virtual desktops, read from where Explorer keeps them (ADR-0011). Windows has no
//! public API that says which desktop is current or when that changes, so this reads the
//! registry the way PowerToys does:
//!
//! - `CurrentVirtualDesktop`, a GUID, under `HKCU\Software\Microsoft\Windows\CurrentVersion\
//!   Explorer\VirtualDesktops`, or on older Windows 10 under `...\Explorer\SessionInfo\
//!   <session>\VirtualDesktops`;
//! - `VirtualDesktopIDs` under the first key, 16 bytes a desktop, in Task View's order;
//! - a renamed desktop's `Name` under `...\VirtualDesktops\Desktops\{GUID}`.
//!
//! Every function answers "nothing" rather than failing: a PC that never had a second
//! desktop has none of these values, and a future Windows may move them again. Workspaces
//! then simply never follow a desktop.

use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, KEY_NOTIFY, REG_NOTIFY_CHANGE_LAST_SET, REG_NOTIFY_CHANGE_NAME, REG_ROUTINE_FLAGS, RRF_RT_REG_BINARY, RRF_RT_REG_SZ, RegCloseKey, RegGetValueW, RegNotifyChangeKeyValue, RegOpenKeyExW};
use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForMultipleObjects};
use windows::core::HSTRING;

const EXPLORER: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer";
const DESKTOPS: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\VirtualDesktops";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Desktop {
    /// `{GUID}`, upper case.
    pub id: String,
    /// Its own name, or "Desktop 2" as Task View calls an unnamed one.
    pub name: String,
}

/// A GUID as Windows writes it, from its 16 bytes in memory order; all zeros is no desktop.
pub fn guid_text(b: &[u8]) -> Option<String> {
    let b: &[u8; 16] = b.get(..16)?.try_into().ok()?;
    if b.iter().all(|x| *x == 0) {
        return None;
    }
    let d1 = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let d2 = u16::from_le_bytes([b[4], b[5]]);
    let d3 = u16::from_le_bytes([b[6], b[7]]);
    Some(format!("{{{d1:08X}-{d2:04X}-{d3:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}", b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]))
}

/// `VirtualDesktopIDs`: one GUID every 16 bytes.
pub fn guid_list(b: &[u8]) -> Vec<String> {
    b.chunks_exact(16).filter_map(guid_text).collect()
}

/// Names for the ids, in order: its own, else "Desktop n" by place.
pub fn named(ids: &[String], name_of: impl Fn(&str) -> Option<String>) -> Vec<Desktop> {
    ids.iter().enumerate().map(|(i, id)| Desktop { id: id.clone(), name: name_of(id).filter(|n| !n.trim().is_empty()).unwrap_or_else(|| format!("Desktop {}", i + 1)) }).collect()
}

fn read(sub: &str, value: &str, kind: REG_ROUTINE_FLAGS) -> Option<Vec<u8>> {
    let (sub, value) = (HSTRING::from(sub), HSTRING::from(value));
    let mut len = 0u32;
    unsafe { RegGetValueW(HKEY_CURRENT_USER, &sub, &value, kind, None, None, Some(&mut len)) }.ok().ok()?;
    let mut buf = vec![0u8; len as usize];
    unsafe { RegGetValueW(HKEY_CURRENT_USER, &sub, &value, kind, None, Some(buf.as_mut_ptr().cast()), Some(&mut len)) }.ok().ok()?;
    buf.truncate(len as usize);
    Some(buf)
}

fn read_text(sub: &str, value: &str) -> Option<String> {
    let b = read(sub, value, RRF_RT_REG_SZ)?;
    let w: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|c| *c != 0).collect();
    Some(String::from_utf16_lossy(&w))
}

/// Older Windows 10 keeps the current desktop per sign-in session.
fn session_key() -> Option<String> {
    use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows::Win32::System::Threading::GetCurrentProcessId;
    let mut session = 0u32;
    unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) }.ok()?;
    Some(format!(r"{EXPLORER}\SessionInfo\{session}\VirtualDesktops"))
}

/// The desktop on screen, or `None` when Windows does not say (one desktop, never split).
pub fn current() -> Option<String> {
    let at = |key: &str| read(key, "CurrentVirtualDesktop", RRF_RT_REG_BINARY).and_then(|b| guid_text(&b));
    at(DESKTOPS).or_else(|| session_key().and_then(|k| at(&k)))
}

/// Every desktop, in Task View's order.
pub fn desktops() -> Vec<Desktop> {
    let ids = read(DESKTOPS, "VirtualDesktopIDs", RRF_RT_REG_BINARY).map(|b| guid_list(&b)).unwrap_or_default();
    named(&ids, |id| read_text(&format!(r"{DESKTOPS}\Desktops\{id}"), "Name"))
}

fn open_for_notify(sub: &str) -> Option<HKEY> {
    let mut key = HKEY::default();
    unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, &HSTRING::from(sub), None, KEY_NOTIFY, &mut key) }.ok().ok()?;
    Some(key)
}

/// Calls `changed` from its own thread whenever the desktops' keys change: a switch, a new
/// desktop, a rename. It waits on the registry, never on a timer, so it costs nothing until
/// then; one switch writes several values, so the caller should wait a moment and compare.
/// Before a second desktop has ever existed it watches Explorer's key for `VirtualDesktops`
/// to appear.
pub fn watch(changed: impl Fn() + Send + 'static) {
    let spawned = std::thread::Builder::new().name("virtual desktops".into()).spawn(move || {
        loop {
            let mut keys: Vec<(HKEY, bool)> = [Some(DESKTOPS.to_string()), session_key()].into_iter().flatten().filter_map(|k| open_for_notify(&k)).map(|k| (k, true)).collect();
            let waiting_for_first = keys.is_empty();
            if waiting_for_first {
                match open_for_notify(EXPLORER) {
                    Some(k) => keys.push((k, false)),
                    None => return,
                }
            }
            let mut events: Vec<HANDLE> = Vec::new();
            for (key, subtree) in &keys {
                let Ok(ev) = (unsafe { CreateEventW(None, false, false, windows::core::PCWSTR::null()) }) else { continue };
                let filter = REG_NOTIFY_CHANGE_LAST_SET | REG_NOTIFY_CHANGE_NAME;
                if unsafe { RegNotifyChangeKeyValue(*key, *subtree, filter, Some(ev), true) }.is_ok() {
                    events.push(ev);
                }
            }
            if events.is_empty() {
                return;
            }
            let n = events.len() as u32;
            let woke = unsafe { WaitForMultipleObjects(&events, false, INFINITE) };
            for (key, _) in keys {
                let _ = unsafe { RegCloseKey(key) };
            }
            for ev in events {
                let _ = unsafe { windows::Win32::Foundation::CloseHandle(ev) };
            }
            if woke.0 >= WAIT_OBJECT_0.0 + n {
                return; // failed or abandoned: stop rather than spin
            }
            if !waiting_for_first || open_for_notify(DESKTOPS).is_some() {
                changed();
            }
        }
    });
    let _ = spawned;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_guid_reads_as_windows_writes_it() {
        // {6E7F1A2B-3C4D-5E6F-8091-A2B3C4D5E6F7} in memory order
        let b = [0x2B, 0x1A, 0x7F, 0x6E, 0x4D, 0x3C, 0x6F, 0x5E, 0x80, 0x91, 0xA2, 0xB3, 0xC4, 0xD5, 0xE6, 0xF7];
        assert_eq!(guid_text(&b).as_deref(), Some("{6E7F1A2B-3C4D-5E6F-8091-A2B3C4D5E6F7}"));
        assert_eq!(guid_text(&[0; 16]), None, "all zeros: the one desktop before any was added");
        assert_eq!(guid_text(&b[..15]), None);
    }

    #[test]
    fn the_desktop_list_is_sixteen_bytes_each_and_unnamed_ones_are_numbered() {
        let mut bytes = vec![1u8; 16];
        bytes.extend([2u8; 16]);
        bytes.extend([9u8; 5]); // a torn tail is ignored
        let ids = guid_list(&bytes);
        assert_eq!(ids.len(), 2);
        let ds = named(&ids, |id| (id == ids[1].as_str()).then(|| "Games".to_string()));
        assert_eq!(ds.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["Desktop 1", "Games"]);
    }

    #[test]
    fn reading_the_registry_never_panics_whatever_it_holds() {
        // a test machine, Wine, or a PC that never split its desktop has no such values
        let (now, all) = (current(), desktops());
        if let Some(id) = now {
            assert!(id.starts_with('{') && id.ends_with('}'), "{id}");
        }
        assert!(all.iter().all(|d| !d.name.is_empty()));
    }
}
