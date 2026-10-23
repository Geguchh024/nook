//! Now-playing info and transport controls through the system media
//! transport controls (SMTC, `GlobalSystemMediaTransportControlsSessionManager`).
//!
//! All WinRT work happens on one worker thread. It subscribes to the
//! manager's and the current session's change events, so it sleeps until
//! something actually changes (plus a slow safety refresh) and costs nothing
//! while the music is idle. Snapshots go to the UI through a channel, and
//! `on_change` wakes the UI.

use crate::{PlatformError, Result, Win32Context};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows::Foundation::TypedEventHandler;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Storage::Streams::DataReader;
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};

/// Album art, decoded and downscaled to at most [`ART_SIZE`] square.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Art {
    pub width: usize,
    pub height: usize,
    /// Unpremultiplied RGBA8.
    pub rgba: Vec<u8>,
}

pub const ART_SIZE: u32 = 96;

/// What is playing right now.
#[derive(Clone, Debug)]
pub struct NowPlaying {
    /// Cleaned-up app id, e.g. "Spotify" or "chrome".
    pub source: String,
    pub title: String,
    pub artist: String,
    pub playing: bool,
    pub can_previous: bool,
    pub can_next: bool,
    pub can_toggle: bool,
    /// Position and duration in seconds when the app reports a timeline.
    pub timeline: Option<(f64, f64)>,
    /// When `timeline` was sampled; a playing position advances from here.
    pub captured: Instant,
    /// Shared between snapshots of the same track.
    pub art: Option<Arc<Art>>,
}

impl NowPlaying {
    /// Position and duration (seconds) extrapolated to now.
    pub fn position(&self) -> Option<(f64, f64)> {
        let (pos, dur) = self.timeline?;
        let pos = if self.playing {
            pos + self.captured.elapsed().as_secs_f64()
        } else {
            pos
        };
        Some((pos.clamp(0.0, dur), dur))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaCommand {
    PlayPause,
    Next,
    Previous,
}

enum Msg {
    Wake,
    Command(MediaCommand),
}

/// Handle to the worker thread. Dropping it stops the worker.
pub struct MediaWatcher {
    tx: Sender<Msg>,
    rx: Receiver<Option<NowPlaying>>,
}

impl MediaWatcher {
    /// Starts the worker. `on_change` runs (on the worker thread) after each
    /// new snapshot.
    pub fn spawn(on_change: impl Fn() + Send + 'static) -> Result<Self> {
        let (tx, worker_rx) = mpsc::channel();
        let (snap_tx, rx) = mpsc::channel();
        let wake = tx.clone();
        std::thread::Builder::new()
            .name("nook-media".into())
            .spawn(move || {
                if let Err(e) = run(&worker_rx, &wake, &snap_tx, &on_change) {
                    tracing::warn!("media controls unavailable: {e}");
                }
            })
            .map_err(|e| PlatformError::Other(format!("spawning media thread: {e}")))?;
        Ok(Self { tx, rx })
    }

    /// Latest snapshot since the last call, if any (`Some(None)` means the
    /// session went away).
    pub fn poll(&self) -> Option<Option<NowPlaying>> {
        self.rx.try_iter().last()
    }

    pub fn send(&self, cmd: MediaCommand) {
        let _ = self.tx.send(Msg::Command(cmd));
    }
}

/// The current session and the event subscriptions on it.
struct Watched {
    id: String,
    session: Session,
    /// Event registration tokens: properties, playback, timeline.
    tokens: [i64; 3],
}

impl Watched {
    fn new(session: Session, id: String, wake: &Sender<Msg>) -> Result<Self> {
        let w = wake.clone();
        let props = session
            .MediaPropertiesChanged(&TypedEventHandler::new(move |_, _| {
                let _ = w.send(Msg::Wake);
                Ok(())
            }))
            .api("MediaPropertiesChanged")?;
        let w = wake.clone();
        let playback = session
            .PlaybackInfoChanged(&TypedEventHandler::new(move |_, _| {
                let _ = w.send(Msg::Wake);
                Ok(())
            }))
            .api("PlaybackInfoChanged")?;
        let w = wake.clone();
        let timeline = session
            .TimelinePropertiesChanged(&TypedEventHandler::new(move |_, _| {
                let _ = w.send(Msg::Wake);
                Ok(())
            }))
            .api("TimelinePropertiesChanged")?;
        Ok(Self {
            id,
            session,
            tokens: [props, playback, timeline],
        })
    }
}

impl Drop for Watched {
    fn drop(&mut self) {
        let [props, playback, timeline] = self.tokens;
        let _ = self.session.RemoveMediaPropertiesChanged(props);
        let _ = self.session.RemovePlaybackInfoChanged(playback);
        let _ = self.session.RemoveTimelinePropertiesChanged(timeline);
    }
}

/// Track identity used to decide whether the art must be reloaded.
type ArtKey = (String, String, String);

fn run(
    rx: &Receiver<Msg>,
    wake: &Sender<Msg>,
    out: &Sender<Option<NowPlaying>>,
    on_change: &dyn Fn(),
) -> Result<()> {
    // SAFETY: initializes WinRT for this worker thread only.
    unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.api("RoInitialize")?;
    let manager = Manager::RequestAsync()
        .api("RequestAsync")?
        .join()
        .api("RequestAsync.join")?;
    let w = wake.clone();
    manager
        .CurrentSessionChanged(&TypedEventHandler::new(move |_, _| {
            let _ = w.send(Msg::Wake);
            Ok(())
        }))
        .api("CurrentSessionChanged")?;

    let mut watched: Option<Watched> = None;
    let mut art: Option<(ArtKey, Option<Arc<Art>>)> = None;
    loop {
        let session = manager.GetCurrentSession().ok();
        let id = session
            .as_ref()
            .and_then(|s| s.SourceAppUserModelId().ok())
            .map(|h| h.to_string());
        if watched.as_ref().map(|w| &w.id) != id.as_ref() {
            watched = match (session, id) {
                (Some(s), Some(id)) => Watched::new(s, id, wake)
                    .inspect_err(|e| tracing::debug!("media session events: {e}"))
                    .ok(),
                _ => None,
            };
        }
        let snapshot = watched
            .as_ref()
            .and_then(|w| match snapshot(&w.session, &w.id, &mut art) {
                Ok(s) => Some(s),
                Err(e) => {
                    tracing::debug!("media snapshot: {e}");
                    None
                }
            });
        if out.send(snapshot).is_err() {
            return Ok(());
        }
        on_change();

        // Sleep until something changes; a slow refresh covers apps that
        // forget to raise events.
        match rx.recv_timeout(Duration::from_secs(20)) {
            Ok(msg) => {
                // Events come in bursts (title, art and playback together).
                std::thread::sleep(Duration::from_millis(40));
                for msg in std::iter::once(msg).chain(rx.try_iter()) {
                    if let (Msg::Command(cmd), Some(w)) = (msg, &watched)
                        && let Err(e) = command(&w.session, cmd)
                    {
                        tracing::debug!("media command {cmd:?}: {e}");
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

fn command(session: &Session, cmd: MediaCommand) -> Result<()> {
    let op = match cmd {
        MediaCommand::PlayPause => session.TryTogglePlayPauseAsync(),
        MediaCommand::Next => session.TrySkipNextAsync(),
        MediaCommand::Previous => session.TrySkipPreviousAsync(),
    }
    .api("TryControlAsync")?;
    op.join().api("TryControlAsync.join")?;
    Ok(())
}

fn snapshot(
    session: &Session,
    id: &str,
    art_cache: &mut Option<(ArtKey, Option<Arc<Art>>)>,
) -> Result<NowPlaying> {
    let props = session
        .TryGetMediaPropertiesAsync()
        .api("TryGetMediaPropertiesAsync")?
        .join()
        .api("TryGetMediaPropertiesAsync.join")?;
    let title = props.Title().map(|h| h.to_string()).unwrap_or_default();
    let artist = props.Artist().map(|h| h.to_string()).unwrap_or_default();
    let album = props
        .AlbumTitle()
        .map(|h| h.to_string())
        .unwrap_or_default();

    let info = session.GetPlaybackInfo().api("GetPlaybackInfo")?;
    let playing = info.PlaybackStatus().ok() == Some(Status::Playing);
    let controls = info.Controls().api("PlaybackInfo.Controls")?;

    let key = (title.clone(), artist.clone(), album);
    let art = match art_cache {
        Some((k, art)) if *k == key => art.clone(),
        _ => {
            let art = load_art(&props)
                .inspect_err(|e| tracing::debug!("album art: {e}"))
                .ok()
                .map(Arc::new);
            *art_cache = Some((key, art.clone()));
            art
        }
    };

    Ok(NowPlaying {
        source: clean_source(id),
        title,
        artist,
        playing,
        can_previous: controls.IsPreviousEnabled().unwrap_or(false),
        can_next: controls.IsNextEnabled().unwrap_or(false),
        can_toggle: controls.IsPlayPauseToggleEnabled().unwrap_or(false),
        timeline: timeline(session, playing),
        captured: Instant::now(),
        art,
    })
}

/// 100 ns ticks per second (WinRT `TimeSpan` / `DateTime` units).
const TICKS: f64 = 10_000_000.0;

fn timeline(session: &Session, playing: bool) -> Option<(f64, f64)> {
    let t = session.GetTimelineProperties().ok()?;
    let start = t.StartTime().ok()?.Duration;
    let end = t.EndTime().ok()?.Duration;
    let pos = t.Position().ok()?.Duration;
    let dur = (end - start) as f64 / TICKS;
    if dur <= 0.0 {
        return None;
    }
    let mut pos = (pos - start) as f64 / TICKS;
    // The position is as of `LastUpdatedTime`; carry it forward to now.
    if playing && let Ok(updated) = t.LastUpdatedTime() {
        let since = (now_ticks() - updated.UniversalTime) as f64 / TICKS;
        if since > 0.0 {
            pos += since;
        }
    }
    Some((pos.clamp(0.0, dur), dur))
}

/// Now as a WinRT `DateTime` (100 ns ticks since 1601-01-01 UTC).
fn now_ticks() -> i64 {
    const EPOCH_DIFF: i64 = 11_644_473_600;
    let unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    (unix.as_secs() as i64 + EPOCH_DIFF) * 10_000_000 + i64::from(unix.subsec_nanos() / 100)
}

fn load_art(
    props: &windows::Media::Control::GlobalSystemMediaTransportControlsSessionMediaProperties,
) -> Result<Art> {
    let thumb = props.Thumbnail().api("Thumbnail")?;
    let stream = thumb
        .OpenReadAsync()
        .api("OpenReadAsync")?
        .join()
        .api("OpenReadAsync.join")?;
    let size = u32::try_from(stream.Size().api("Size")?)
        .map_err(|_| PlatformError::Other("album art too large".into()))?;
    let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0).api("GetInputStreamAt")?)
        .api("CreateDataReader")?;
    reader
        .LoadAsync(size)
        .api("LoadAsync")?
        .join()
        .api("LoadAsync.join")?;
    let mut bytes = vec![0u8; size as usize];
    reader.ReadBytes(&mut bytes).api("ReadBytes")?;
    let image = image::load_from_memory(&bytes)
        .map_err(|e| PlatformError::Other(format!("decoding album art: {e}")))?
        .thumbnail(ART_SIZE, ART_SIZE)
        .to_rgba8();
    Ok(Art {
        width: image.width() as usize,
        height: image.height() as usize,
        rgba: image.into_raw(),
    })
}

/// "SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify" -> "Spotify",
/// "chrome.exe" -> "chrome". Opaque ids (mostly hex, e.g. some browsers'
/// per-tab ids) come back empty rather than as noise.
pub fn clean_source(id: &str) -> String {
    let name = id.rsplit('!').next().unwrap_or(id);
    let name = name.strip_suffix(".exe").unwrap_or(name);
    let name = name.rsplit(['\\', '/']).next().unwrap_or(name);
    let hex = name.chars().filter(char::is_ascii_hexdigit).count();
    if name.len() >= 8 && hex * 3 >= name.len() * 2 {
        return String::new();
    }
    name.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_names() {
        assert_eq!(
            clean_source("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify"),
            "Spotify"
        );
        assert_eq!(clean_source("chrome.exe"), "chrome");
        assert_eq!(clean_source("C:\\Apps\\foo.exe"), "foo");
        assert_eq!(clean_source("F0DC299D809B9700"), "");
    }

    #[test]
    fn paused_position_does_not_advance() {
        let np = NowPlaying {
            source: String::new(),
            title: String::new(),
            artist: String::new(),
            playing: false,
            can_previous: false,
            can_next: false,
            can_toggle: false,
            timeline: Some((30.0, 100.0)),
            captured: Instant::now() - Duration::from_secs(5),
            art: None,
        };
        assert_eq!(np.position(), Some((30.0, 100.0)));
        let playing = NowPlaying {
            playing: true,
            ..np
        };
        let (pos, _) = playing.position().unwrap();
        assert!(pos >= 35.0);
    }
}
