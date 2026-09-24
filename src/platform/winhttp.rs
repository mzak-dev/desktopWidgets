//! `Fetch` over WinHTTP: the system's TLS, certificates and proxy, and no HTTP crate.
//! Redirects, cookies and Windows credentials are all off (ADR-0008).

use std::ffi::c_void;
use std::time::{Duration, Instant};

use windows::Win32::Networking::WinHttp::*;
use windows::core::{HSTRING, PCWSTR};

use crate::net::{BODY_CAP, Fetch, Request, Response, Target};

const DEADLINE: Duration = Duration::from_secs(15);

struct Handle(*mut c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

fn opened(h: *mut c_void, what: &str) -> Result<Handle, String> {
    if h.is_null() { Err(format!("{what}: {}", windows::core::Error::from_thread())) } else { Ok(Handle(h)) }
}

fn set_u32(h: *mut c_void, option: u32, value: u32) {
    unsafe {
        let _ = WinHttpSetOption(Some(h as *const c_void), option, Some(&value.to_ne_bytes()));
    }
}

/// One WinHTTP session for every plugin; WinHTTP handles are safe to share across threads.
pub struct WinHttp {
    session: Handle,
}

unsafe impl Send for WinHttp {}
unsafe impl Sync for WinHttp {}

impl WinHttp {
    pub fn new() -> Result<WinHttp, String> {
        let agent = HSTRING::from(format!("Wayfinder/{}", env!("CARGO_PKG_VERSION")));
        let session = opened(unsafe { WinHttpOpen(&agent, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0) }, "WinHttpOpen")?;
        unsafe {
            WinHttpSetTimeouts(session.0, 5000, 5000, 5000, 10_000).map_err(|e| e.to_string())?;
        }
        set_u32(session.0, WINHTTP_OPTION_REDIRECT_POLICY, WINHTTP_OPTION_REDIRECT_POLICY_NEVER);
        set_u32(session.0, WINHTTP_OPTION_DECOMPRESSION, WINHTTP_DECOMPRESSION_FLAG_GZIP | WINHTTP_DECOMPRESSION_FLAG_DEFLATE);
        Ok(WinHttp { session })
    }
}

fn header_text(req: *mut c_void, query: u32) -> Option<String> {
    let mut buf = [0u16; 2048];
    let mut len = (buf.len() * 2) as u32;
    let ok = unsafe { WinHttpQueryHeaders(req, query, PCWSTR::null(), Some(buf.as_mut_ptr().cast()), &mut len, std::ptr::null_mut()) };
    ok.ok().map(|_| String::from_utf16_lossy(&buf[..len as usize / 2]))
}

impl Fetch for WinHttp {
    fn fetch(&self, to: &Target, r: &Request) -> Result<Response, String> {
        let started = Instant::now();
        let connect = opened(unsafe { WinHttpConnect(self.session.0, &HSTRING::from(to.host.as_str()), 443, 0) }, "connect")?;
        let req = opened(
            unsafe { WinHttpOpenRequest(connect.0, &HSTRING::from(r.method.as_str()), &HSTRING::from(to.path.as_str()), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), WINHTTP_FLAG_SECURE) },
            "request",
        )?;
        set_u32(req.0, WINHTTP_OPTION_DISABLE_FEATURE, WINHTTP_DISABLE_COOKIES | WINHTTP_DISABLE_AUTHENTICATION | WINHTTP_DISABLE_REDIRECTS);
        set_u32(req.0, WINHTTP_OPTION_AUTOLOGON_POLICY, WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH);
        let headers: Vec<u16> = r.headers.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect::<String>().encode_utf16().collect();
        let body = r.body.as_deref().unwrap_or("").as_bytes();
        unsafe {
            WinHttpSendRequest(req.0, (!headers.is_empty()).then_some(&headers[..]), (!body.is_empty()).then_some(body.as_ptr().cast()), body.len() as u32, body.len() as u32, 0).map_err(|e| format!("could not reach {}: {e}", to.host))?;
            WinHttpReceiveResponse(req.0, std::ptr::null_mut()).map_err(|e| format!("no answer from {}: {e}", to.host))?;
        }
        let mut status = 0u32;
        let mut size = 4u32;
        unsafe {
            WinHttpQueryHeaders(req.0, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, PCWSTR::null(), Some((&mut status as *mut u32).cast()), &mut size, std::ptr::null_mut()).map_err(|e| e.to_string())?;
        }
        let mut body = Vec::new();
        loop {
            if started.elapsed() > DEADLINE {
                return Err(format!("{} took longer than {} s", to.host, DEADLINE.as_secs()));
            }
            let mut avail = 0u32;
            unsafe { WinHttpQueryDataAvailable(req.0, &mut avail) }.map_err(|e| e.to_string())?;
            if avail == 0 {
                break;
            }
            if body.len() + avail as usize > BODY_CAP {
                return Err(format!("the response is over {} MB", BODY_CAP >> 20));
            }
            let start = body.len();
            body.resize(start + avail as usize, 0);
            let mut read = 0u32;
            unsafe { WinHttpReadData(req.0, body[start..].as_mut_ptr().cast(), avail, &mut read) }.map_err(|e| e.to_string())?;
            body.truncate(start + read as usize);
        }
        Ok(Response {
            status: status as u16,
            content_type: header_text(req.0, WINHTTP_QUERY_CONTENT_TYPE).unwrap_or_default(),
            location: header_text(req.0, WINHTTP_QUERY_LOCATION),
            body: String::from_utf8_lossy(&body).into_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs the internet: `cargo test --lib -- --ignored winhttp`.
    #[test]
    #[ignore]
    fn winhttp_fetches_a_real_page() {
        let w = WinHttp::new().unwrap();
        let to = Target { host: "api.open-meteo.com".into(), path: "/v1/forecast?latitude=52.52&longitude=13.41&current=temperature_2m".into() };
        let r = w.fetch(&to, &Request { method: "GET".into(), ..Default::default() }).unwrap();
        assert_eq!(r.status, 200, "{}", r.body);
        assert!(r.body.contains("temperature_2m"));
    }
}
