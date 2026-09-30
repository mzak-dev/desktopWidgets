//! The real capture: WASAPI loopback on the default output, mixed down to mono. The thread
//! that polls it (`data::audio`) is the only one that calls it, so the COM objects stay on it.

use std::sync::Mutex;

use windows::Win32::Media::Audio::{AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, IAudioCaptureClient, IAudioClient, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, WAVEFORMATEX, WAVEFORMATEXTENSIBLE, eConsole, eRender};
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree};

use super::capture::{Capture, to_mono};

/// One open capture session.
struct Stream {
    devices: IMMDeviceEnumerator,
    device_id: String,
    client: IAudioClient,
    cap: IAudioCaptureClient,
    channels: usize,
    float: bool,
    frame_bytes: usize,
}

// SAFETY: a session is opened, read and closed by one thread at a time (the analysis thread);
// the mutex around it only hands it over between sessions.
unsafe impl Send for Stream {}

/// Captures what the default output plays. Nothing is opened until `open`.
#[derive(Default)]
pub struct Wasapi {
    stream: Mutex<Option<Stream>>,
}

fn id_of(d: &IMMDevice) -> String {
    unsafe {
        let Ok(p) = d.GetId() else { return String::new() };
        let s = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

fn start() -> windows::core::Result<(Stream, u32)> {
    // COM is per thread: the first session on a thread starts it for good
    thread_local! {
        static COM: () = unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        };
    }
    COM.with(|_| {});
    unsafe {
        let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = devices.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let device_id = id_of(&device);
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
        let fmt = client.GetMixFormat()?;
        let f: WAVEFORMATEX = std::ptr::read_unaligned(fmt);
        let (tag, bits, channels, rate) = (f.wFormatTag, f.wBitsPerSample, f.nChannels as usize, f.nSamplesPerSec);
        // WAVE_FORMAT_IEEE_FLOAT, or WAVE_FORMAT_EXTENSIBLE naming float
        let float = tag == 3 || (tag == 0xFFFE && std::ptr::read_unaligned(fmt as *const WAVEFORMATEXTENSIBLE).SubFormat.data1 == 3);
        let init = client.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, 200_000, 0, fmt, None);
        CoTaskMemFree(Some(fmt as *const _));
        init?;
        if !(float && bits == 32 || !float && bits == 16) {
            return Err(windows::core::Error::new(windows::Win32::Foundation::E_FAIL, format!("an output format it cannot read ({bits}-bit)")));
        }
        let cap: IAudioCaptureClient = client.GetService()?;
        client.Start()?;
        Ok((Stream { devices, device_id, client, cap, channels, float, frame_bytes: channels * bits as usize / 8 }, rate))
    }
}

impl Stream {
    /// Appends every packet waiting to `out`, as mono.
    fn drain(&self, out: &mut Vec<f32>) -> windows::core::Result<()> {
        unsafe {
            loop {
                if self.cap.GetNextPacketSize()? == 0 {
                    return Ok(());
                }
                let (mut data, mut frames, mut flags) = (std::ptr::null_mut(), 0u32, 0u32);
                self.cap.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                    out.extend(std::iter::repeat_n(0.0, frames as usize));
                } else {
                    out.extend(to_mono(std::slice::from_raw_parts(data, frames as usize * self.frame_bytes), self.channels, self.float));
                }
                self.cap.ReleaseBuffer(frames)?;
            }
        }
    }

    /// The default output is another device than this session opened.
    fn moved(&self) -> bool {
        unsafe { self.devices.GetDefaultAudioEndpoint(eRender, eConsole).map(|d| id_of(&d)).is_ok_and(|id| id != self.device_id) }
    }
}

impl Capture for Wasapi {
    fn open(&self) -> Result<u32, String> {
        let (stream, rate) = start().map_err(|e| e.to_string())?;
        *self.stream.lock().unwrap_or_else(|e| e.into_inner()) = Some(stream);
        Ok(rate)
    }

    fn read(&self, out: &mut Vec<f32>) -> Result<(), String> {
        let stream = self.stream.lock().unwrap_or_else(|e| e.into_inner());
        stream.as_ref().ok_or("capture is not open")?.drain(out).map_err(|e| e.to_string())
    }

    fn moved(&self) -> bool {
        self.stream.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(Stream::moved)
    }

    fn close(&self) {
        if let Some(s) = self.stream.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = unsafe { s.client.Stop() };
        }
    }
}
