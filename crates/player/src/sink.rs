//! The engine's output, shared (fastframe's) or exclusive, and changing it under a track.

use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;

use fastframe_audio::{Device, Output, OutputOptions};

use super::{Engine, OutputConfig, PlayerEvent};
use crate::exclusive::Exclusive;
use crate::{Format, Renderer};

pub(super) enum Sink {
    Shared(Box<Output<Renderer>>),
    Exclusive(Exclusive),
}

impl Sink {
    pub(super) fn is_paused(&self) -> bool {
        match self {
            Sink::Shared(out) => out.is_paused(),
            Sink::Exclusive(x) => x.is_paused(),
        }
    }

    pub(super) fn pause(&mut self) {
        match self {
            Sink::Shared(out) => out.pause(),
            Sink::Exclusive(x) => x.pause(),
        }
    }

    pub(super) fn resume(&mut self) {
        match self {
            Sink::Shared(out) => out.resume(),
            Sink::Exclusive(x) => x.resume(),
        }
    }
}

pub(super) fn shared_options(device: &Device) -> OutputOptions {
    OutputOptions {
        device: device.clone(),
        ..OutputOptions::default()
    }
}

impl Engine {
    /// A renderer for a new output: it carries on the clock.
    pub(super) fn renderer(&self) -> Renderer {
        Renderer::new(Arc::clone(&self.shared), self.ring_tx.clone())
    }

    /// Before a track in `format` starts: keeps a sink that suits it (exclusive at that
    /// format, or shared for a shared config), else opens one.
    pub(super) fn use_output(&mut self, format: Format) {
        let keep = match &self.sink {
            Some(Sink::Exclusive(x)) => self.config.exclusive && x.format == format,
            Some(Sink::Shared(_)) => !self.config.exclusive,
            None => self.manual && !self.config.exclusive, // the caller's renderer
        };
        if !keep {
            self.open_output(Some(format));
        }
    }

    /// A new config moves a loaded track to it at once, from its position. With no track,
    /// shared output opens on the device; exclusive waits for the next Load, at its format.
    pub(super) fn set_output(&mut self, config: OutputConfig) {
        if config != self.config {
            self.config = config;
            let heard = self.ending.as_ref().or(self.track.as_ref());
            self.open_output(heard.map(|t| t.decoder.format()));
        }
    }

    /// Closes the sink and opens the one the config asks for: exclusive at `format` (shared
    /// when the device refuses), or shared.
    fn open_output(&mut self, format: Option<Format>) {
        // Let the device go first: some take one stream, and on Windows any shared stream
        // blocks an exclusive one.
        self.sink = None;
        match format.filter(|_| self.config.exclusive) {
            Some(format) => {
                let renderer = self.renderer();
                match (self.open_exclusive)(&self.config.device, format, renderer) {
                    Ok(x) => self.sink = Some(Sink::Exclusive(x)),
                    Err(reason) => self.fallback(reason),
                }
            }
            None => self.open_shared(),
        }
        self.sink_changed();
    }

    /// Opens shared output in place of none. In manual mode the caller's renderer is the
    /// shared output: nothing to open.
    pub(super) fn open_shared(&mut self) {
        if self.manual {
            return;
        }
        match Output::open(shared_options(&self.config.device), self.renderer()) {
            Ok(out) => {
                self.sink = Some(Sink::Shared(Box::new(out)));
                self.output_failed(None);
            }
            Err(e) => self.output_failed(Some(e)),
        }
    }

    /// Exclusive was refused or lost: shared output plays instead, and the app hears why.
    pub(super) fn fallback(&mut self, reason: String) {
        log::warn!("exclusive output: {reason}; playing shared");
        self.open_shared();
        self.emit(PlayerEvent::Fallback(reason));
    }

    /// The sink changed: exclusive plays at full scale, and a track goes on through the new
    /// renderer from its position (`adopt_ring` re-seeks it there and tells the app).
    pub(super) fn sink_changed(&mut self) {
        let exclusive = matches!(self.sink, Some(Sink::Exclusive(_)));
        self.shared.exclusive.store(exclusive, Relaxed);
        if !self.adopt_ring() {
            self.format_event(); // manual mode's shared output: no new renderer, no new ring
        }
    }
}
