//! The events scripts handle.

/// A function a script may export. The discriminants are bit positions in a script's
/// handler mask and indices into `HANDLERS` in `scripts.js`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Handler {
    /// `init(value)`, once after every script is attached.
    Init = 0,
    /// `update(value)`, every frame.
    Update = 1,
    /// `applyUserProperties(changed)`.
    UserProperties = 2,
    /// `cursorEnter(event)`.
    CursorEnter = 3,
    /// `cursorLeave(event)`.
    CursorLeave = 4,
    /// `cursorMove(event)`.
    CursorMove = 5,
    /// `cursorDown(event)`.
    CursorDown = 6,
    /// `cursorUp(event)`.
    CursorUp = 7,
    /// `cursorClick(event)`.
    CursorClick = 8,
    /// `destroy()`.
    Destroy = 9,
    /// `resizeScreen(size)`.
    ResizeScreen = 10,
    /// `mediaStatusChanged(event)`.
    MediaStatus = 11,
    /// `mediaPlaybackChanged(event)`.
    MediaPlayback = 12,
    /// `mediaPropertiesChanged(event)`.
    MediaProperties = 13,
    /// `mediaThumbnailChanged(event)`.
    MediaThumbnail = 14,
    /// `mediaTimelineChanged(event)`.
    MediaTimeline = 15,
    /// `animationEvent(event, value)`.
    AnimationEvent = 16,
    /// `applyGeneralSettings(changed)`.
    GeneralSettings = 17,
}

impl Handler {
    pub(super) fn bit(self) -> u32 {
        1 << self as u8
    }
}

/// What the cursor did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorEvent {
    /// It came over a layer.
    Enter,
    /// It left a layer.
    Leave,
    /// It moved.
    Move,
    /// The left button went down over a layer.
    Down,
    /// The left button came up over a layer.
    Up,
    /// The left button went down and came up over the same layer.
    Click,
}

impl CursorEvent {
    pub(super) fn handler(self) -> Handler {
        match self {
            CursorEvent::Enter => Handler::CursorEnter,
            CursorEvent::Leave => Handler::CursorLeave,
            CursorEvent::Move => Handler::CursorMove,
            CursorEvent::Down => Handler::CursorDown,
            CursorEvent::Up => Handler::CursorUp,
            CursorEvent::Click => Handler::CursorClick,
        }
    }
}

/// What the media player of the desktop did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MediaEvent<'a> {
    /// Media integration was turned on or off.
    Status {
        /// Whether it is on now.
        enabled: bool,
    },
    /// Playback started, paused or stopped.
    Playback {
        /// `MediaPlaybackEvent.PLAYBACK_STOPPED` (0), `PLAYBACK_PLAYING` (1) or
        /// `PLAYBACK_PAUSED` (2).
        state: u8,
    },
    /// Another track.
    Properties {
        /// Track title.
        title: &'a str,
        /// Artist.
        artist: &'a str,
        /// Track subtitle.
        sub_title: &'a str,
        /// Album title.
        album_title: &'a str,
        /// Album artist.
        album_artist: &'a str,
        /// Genres, separated by commas.
        genres: &'a str,
        /// `audio` or `video`.
        content_type: &'a str,
    },
    /// Another cover picture, with the colours taken from it as normalised RGB.
    Thumbnail {
        /// Whether there is a picture.
        has_thumbnail: bool,
        /// The dominant colour.
        primary: [f64; 3],
        /// The second colour.
        secondary: [f64; 3],
        /// The third colour.
        tertiary: [f64; 3],
        /// A colour readable on the primary one.
        text: [f64; 3],
        /// Black or white, whichever stands out more on the primary one.
        high_contrast: [f64; 3],
    },
    /// The position within the track moved.
    Timeline {
        /// Seconds played.
        position: f64,
        /// Seconds the track lasts.
        duration: f64,
    },
}

impl MediaEvent<'_> {
    pub(super) fn handler(&self) -> Handler {
        match self {
            MediaEvent::Status { .. } => Handler::MediaStatus,
            MediaEvent::Playback { .. } => Handler::MediaPlayback,
            MediaEvent::Properties { .. } => Handler::MediaProperties,
            MediaEvent::Thumbnail { .. } => Handler::MediaThumbnail,
            MediaEvent::Timeline { .. } => Handler::MediaTimeline,
        }
    }
}
