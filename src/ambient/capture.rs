//! What the speakers play, as the engine reads it: mono samples from an output device. The
//! Windows adapter is WASAPI loopback; the fixed ones make a steady tone or silence. The
//! analysis (`data::audio`) polls a `Capture` and never sees the device.

/// A source of mono samples. Shared, so it holds its own state behind `&self`; the analysis
/// drives one session at a time: `open`, `read` again and again, `close`.
pub trait Capture: Send + Sync {
    /// Opens the default output for capture and returns its sample rate in Hz.
    fn open(&self) -> Result<u32, String>;
    /// Appends the mono samples that arrived since the last read (none when none did) to `out`.
    fn read(&self, out: &mut Vec<f32>) -> Result<(), String>;
    /// The default output is no longer the one `open` took: close and open again.
    fn moved(&self) -> bool {
        false
    }
    /// Ends the session.
    fn close(&self) {}
    /// Whether samples arrive in real time from a device, so the analysis polls on a thread
    /// while a widget reads it. A synthetic source answers `false` and is read on demand.
    fn realtime(&self) -> bool {
        true
    }
}

/// Interleaved samples to mono, as 32-bit float or 16-bit PCM.
pub fn to_mono(bytes: &[u8], channels: usize, float: bool) -> Vec<f32> {
    let channels = channels.max(1);
    let samples: Vec<f32> = if float {
        bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
    } else {
        bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect()
    };
    samples.chunks_exact(channels).map(|f| f.iter().sum::<f32>() / channels as f32).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_and_16_bit_mix_down_to_mono() {
        let f: Vec<u8> = [0.5f32, -0.5, 1.0, 0.0].iter().flat_map(|x| x.to_le_bytes()).collect();
        assert_eq!(to_mono(&f, 2, true), [0.0, 0.5]);
        let i: Vec<u8> = [16384i16, 16384].iter().flat_map(|x| x.to_le_bytes()).collect();
        assert_eq!(to_mono(&i, 1, false), [0.5, 0.5]);
    }
}
