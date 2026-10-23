//! Voice memos: recording the default microphone (waveIn, 16-bit mono
//! PCM), reading and writing WAV files, and playing them (PlaySound).
//!
//! Recording polls a ring of buffers instead of taking callbacks, so it
//! needs no window and no callback thread; the caller runs it on a thread
//! of its own and stops it with a flag.

use crate::{PlatformError, Result, wide};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use windows::Win32::Media::Audio::{
    CALLBACK_NULL, HWAVEIN, PlaySoundW, SND_ASYNC, SND_FILENAME, SND_FLAGS, SND_NODEFAULT,
    WAVE_FORMAT_PCM, WAVE_MAPPER, WAVEFORMATEX, WAVEHDR, WHDR_DONE, waveInAddBuffer, waveInClose,
    waveInGetNumDevs, waveInOpen, waveInPrepareHeader, waveInReset, waveInStart,
    waveInUnprepareHeader,
};
use windows::core::{PCWSTR, PSTR};

pub const SAMPLE_RATE: u32 = 44_100;
/// Samples per buffer: 50 ms.
const CHUNK: usize = SAMPLE_RATE as usize / 20;
const BUFFERS: usize = 8;

fn mm(call: &'static str, code: u32) -> Result<()> {
    if code == 0 {
        Ok(())
    } else {
        Err(PlatformError::Code { call, code })
    }
}

/// Records from the default microphone until `stop` is set, handing each
/// buffer of samples (about 50 ms) to `on_chunk` in order. Blocks.
pub fn record(stop: &AtomicBool, mut on_chunk: impl FnMut(&[i16])) -> Result<()> {
    // SAFETY: the headers and their sample buffers live until after
    // `waveInUnprepareHeader`, neither moves while the device owns them,
    // and a buffer is read only once the driver has marked it done.
    unsafe {
        if waveInGetNumDevs() == 0 {
            return Err(PlatformError::Other("No microphone found".into()));
        }
        let fmt = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_PCM as u16,
            nChannels: 1,
            nSamplesPerSec: SAMPLE_RATE,
            nAvgBytesPerSec: SAMPLE_RATE * 2,
            nBlockAlign: 2,
            wBitsPerSample: 16,
            cbSize: 0,
        };
        let mut hwi = HWAVEIN::default();
        mm(
            "waveInOpen",
            waveInOpen(Some(&mut hwi), WAVE_MAPPER, &fmt, None, None, CALLBACK_NULL),
        )?;
        let mut data: Vec<Box<[i16]>> = (0..BUFFERS)
            .map(|_| vec![0i16; CHUNK].into_boxed_slice())
            .collect();
        let mut headers: Box<[WAVEHDR]> = data
            .iter_mut()
            .map(|d| WAVEHDR {
                lpData: PSTR(d.as_mut_ptr().cast()),
                dwBufferLength: (CHUNK * 2) as u32,
                ..Default::default()
            })
            .collect();
        let size = std::mem::size_of::<WAVEHDR>() as u32;
        for h in headers.iter_mut() {
            waveInPrepareHeader(hwi, h, size);
            waveInAddBuffer(hwi, h, size);
        }
        let started = mm("waveInStart", waveInStart(hwi));
        let mut next = 0;
        while started.is_ok() && !stop.load(Ordering::Relaxed) {
            let h = &mut headers[next];
            if h.dwFlags & WHDR_DONE != 0 {
                let n = (h.dwBytesRecorded as usize / 2).min(CHUNK);
                on_chunk(&data[next][..n]);
                h.dwBytesRecorded = 0;
                waveInAddBuffer(hwi, h, size);
                next = (next + 1) % BUFFERS;
            } else {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        // Reset hands back every queued buffer, the partial one included.
        waveInReset(hwi);
        for _ in 0..BUFFERS {
            let h = &headers[next];
            if h.dwFlags & WHDR_DONE != 0 && h.dwBytesRecorded > 0 {
                let n = (h.dwBytesRecorded as usize / 2).min(CHUNK);
                on_chunk(&data[next][..n]);
            }
            next = (next + 1) % BUFFERS;
        }
        for h in headers.iter_mut() {
            waveInUnprepareHeader(hwi, h, size);
        }
        waveInClose(hwi);
        started
    }
}

/// Writes 16-bit mono PCM WAV, patching the sizes on [`WavWriter::finish`].
pub struct WavWriter {
    file: std::io::BufWriter<std::fs::File>,
    samples: u32,
}

fn header(samples: u32) -> [u8; 44] {
    let data = samples * 2;
    let mut h = [0u8; 44];
    let mut put = |at: usize, bytes: &[u8]| h[at..at + bytes.len()].copy_from_slice(bytes);
    put(0, b"RIFF");
    put(4, &(36 + data).to_le_bytes());
    put(8, b"WAVEfmt ");
    put(16, &16u32.to_le_bytes());
    put(20, &1u16.to_le_bytes()); // PCM
    put(22, &1u16.to_le_bytes()); // mono
    put(24, &SAMPLE_RATE.to_le_bytes());
    put(28, &(SAMPLE_RATE * 2).to_le_bytes());
    put(32, &2u16.to_le_bytes());
    put(34, &16u16.to_le_bytes());
    put(36, b"data");
    put(40, &data.to_le_bytes());
    h
}

impl WavWriter {
    pub fn create(path: &Path) -> std::io::Result<Self> {
        let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
        file.write_all(&header(0))?;
        Ok(Self { file, samples: 0 })
    }

    pub fn write(&mut self, samples: &[i16]) -> std::io::Result<()> {
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        self.file.write_all(&bytes)?;
        self.samples += samples.len() as u32;
        Ok(())
    }

    pub fn finish(mut self) -> std::io::Result<()> {
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(&header(self.samples))?;
        self.file.flush()
    }
}

/// Sample rate and samples (first channel) of a 16-bit PCM WAV file.
pub fn read_wav(path: &Path) -> std::io::Result<(u32, Vec<i16>)> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?.read_to_end(&mut bytes)?;
    let bad = || std::io::Error::new(std::io::ErrorKind::InvalidData, "not a PCM WAV file");
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(bad());
    }
    let u16_at = |i: usize| u16::from_le_bytes([bytes[i], bytes[i + 1]]);
    let u32_at =
        |i: usize| u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
    let (mut rate, mut channels, mut bits) = (0, 1, 16);
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let len = u32_at(at + 4) as usize;
        let body = at + 8;
        if id == b"fmt " && body + 16 <= bytes.len() {
            channels = u16_at(body + 2).max(1) as usize;
            rate = u32_at(body + 4);
            bits = u16_at(body + 14);
        } else if id == b"data" {
            if bits != 16 {
                return Err(bad());
            }
            let end = (body + len).min(bytes.len());
            let samples = bytes[body..end]
                .chunks_exact(2 * channels)
                .map(|c| i16::from_le_bytes([c[0], c[1]]))
                .collect();
            return Ok((rate, samples));
        }
        at = body + len + (len & 1);
    }
    Err(bad())
}

/// Starts playing a WAV file, replacing any sound playing.
pub fn play(path: &Path) -> Result<()> {
    let w = wide(&path.to_string_lossy());
    // SAFETY: a null-terminated path; PlaySound copies what it needs.
    let ok = unsafe {
        PlaySoundW(
            PCWSTR(w.as_ptr()),
            None,
            SND_FILENAME | SND_ASYNC | SND_NODEFAULT,
        )
    };
    if ok.as_bool() {
        Ok(())
    } else {
        Err(PlatformError::Other(format!(
            "could not play {}",
            path.display()
        )))
    }
}

/// Stops what [`play`] started.
pub fn stop_playback() {
    // SAFETY: a null sound stops the current one.
    let _ = unsafe { PlaySoundW(PCWSTR::null(), None, SND_FLAGS(0)) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_round_trip() {
        let path = std::env::temp_dir().join(format!("nook-wav-{}.wav", std::process::id()));
        let mut w = WavWriter::create(&path).unwrap();
        w.write(&[0, 1000, -1000, i16::MAX]).unwrap();
        w.write(&[5]).unwrap();
        w.finish().unwrap();
        let (rate, samples) = read_wav(&path).unwrap();
        assert_eq!(rate, SAMPLE_RATE);
        assert_eq!(samples, [0, 1000, -1000, i16::MAX, 5]);
        let _ = std::fs::remove_file(&path);
    }
}
