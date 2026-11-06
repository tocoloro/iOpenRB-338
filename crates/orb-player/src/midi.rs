//! MIDI clock mailbox between a MIDI callback and the audio callback.
//!
//! The MIDI thread posts tempo, song position and start, stop or continue.
//! [`Driver::apply`] runs inside [`crate::Audio::call`]. It reads atomics only:
//! no lock and no allocation, same as the command queue. A port that is off
//! simply posts nothing.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};

use orb_engine::frame::Engine;

/// No MIDI port is followed.
pub const PORT_OFF: u8 = 0;
/// The app's own virtual port (the one a host such as AUM sends to).
pub const PORT_VIRTUAL: u8 = 1;
/// One external source, named by [`MidiSync::source`].
pub const PORT_SOURCE: u8 = 2;

const TRANSPORT_START: u8 = 1;
const TRANSPORT_STOP: u8 = 2;
const TRANSPORT_CONTINUE: u8 = 3;

/// The latest MIDI clock state. Every method is `&self` and lock-free.
#[derive(Debug)]
pub struct MidiSync {
    kind: AtomicU8,
    source: AtomicU32,
    tempo_milli: AtomicU32,
    tempo_gen: AtomicU64,
    spp_ticks: AtomicU32,
    spp_gen: AtomicU64,
    transport: AtomicU8,
    transport_gen: AtomicU64,
}

impl MidiSync {
    pub fn new() -> Self {
        Self {
            kind: AtomicU8::new(PORT_OFF),
            source: AtomicU32::new(0),
            tempo_milli: AtomicU32::new(0),
            tempo_gen: AtomicU64::new(0),
            spp_ticks: AtomicU32::new(0),
            spp_gen: AtomicU64::new(0),
            transport: AtomicU8::new(0),
            transport_gen: AtomicU64::new(0),
        }
    }

    pub fn set_off(&self) {
        self.kind.store(PORT_OFF, Ordering::Release);
    }

    pub fn set_virtual(&self) {
        self.kind.store(PORT_VIRTUAL, Ordering::Release);
    }

    /// Follow the external source `id` (CoreMIDI unique id, as a bit pattern).
    pub fn set_source(&self, id: u32) {
        self.source.store(id, Ordering::Relaxed);
        self.kind.store(PORT_SOURCE, Ordering::Release);
    }

    pub fn kind(&self) -> u8 {
        self.kind.load(Ordering::Acquire)
    }

    pub fn source(&self) -> u32 {
        self.source.load(Ordering::Relaxed)
    }

    pub fn accepts_virtual(&self) -> bool {
        self.kind() == PORT_VIRTUAL
    }

    pub fn accepts_source(&self, id: u32) -> bool {
        self.kind() == PORT_SOURCE && self.source() == id
    }

    /// Tempo in BPM times 1000. Zero means the clock has not settled yet.
    pub fn set_tempo(&self, milli: u32) {
        self.tempo_milli.store(milli, Ordering::Relaxed);
        self.tempo_gen.fetch_add(1, Ordering::Release);
    }

    /// Song position, in ticks (192 per quarter note).
    pub fn locate(&self, ticks: u32) {
        self.spp_ticks.store(ticks, Ordering::Relaxed);
        self.spp_gen.fetch_add(1, Ordering::Release);
    }

    /// MIDI Start: the next apply locates to 0 and plays.
    pub fn start(&self) {
        self.transport.store(TRANSPORT_START, Ordering::Relaxed);
        self.transport_gen.fetch_add(1, Ordering::Release);
    }

    /// MIDI Stop: the next apply halts and leaves the position.
    pub fn stop(&self) {
        self.transport.store(TRANSPORT_STOP, Ordering::Relaxed);
        self.transport_gen.fetch_add(1, Ordering::Release);
    }

    /// MIDI Continue: the next apply plays from the current position.
    pub fn cont(&self) {
        self.transport.store(TRANSPORT_CONTINUE, Ordering::Relaxed);
        self.transport_gen.fetch_add(1, Ordering::Release);
    }

    /// BPM times 1000 last posted by a clock, or 0 before the clock has settled.
    pub fn tempo(&self) -> u32 {
        self.tempo_milli.load(Ordering::Relaxed)
    }
}

impl Default for MidiSync {
    fn default() -> Self {
        Self::new()
    }
}

/// What the audio callback has already applied. One per stream.
pub(crate) struct Driver {
    sync: Arc<MidiSync>,
    tempo_gen: u64,
    spp_gen: u64,
    transport_gen: u64,
}

impl Driver {
    pub(crate) fn new(sync: Arc<MidiSync>) -> Self {
        Self {
            sync,
            tempo_gen: 0,
            spp_gen: 0,
            transport_gen: 0,
        }
    }

    /// The engine calls are the same ones the command path already runs from this callback.
    pub(crate) fn apply(&mut self, engine: &mut Engine) {
        let tempo_gen = self.sync.tempo_gen.load(Ordering::Acquire);
        if tempo_gen != self.tempo_gen {
            self.tempo_gen = tempo_gen;
            let milli = self.sync.tempo_milli.load(Ordering::Relaxed);
            if (1_000..=999_000).contains(&milli) {
                engine.set_tempo(milli);
            }
        }
        let spp_gen = self.sync.spp_gen.load(Ordering::Acquire);
        if spp_gen != self.spp_gen {
            self.spp_gen = spp_gen;
            engine.locate(self.sync.spp_ticks.load(Ordering::Relaxed));
        }
        let transport_gen = self.sync.transport_gen.load(Ordering::Acquire);
        if transport_gen != self.transport_gen {
            self.transport_gen = transport_gen;
            match self.sync.transport.load(Ordering::Relaxed) {
                TRANSPORT_START => {
                    engine.locate(0);
                    engine.play();
                }
                TRANSPORT_STOP => engine.halt(),
                TRANSPORT_CONTINUE => engine.play(),
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use orb_engine::tables::Tables;

    use crate::{CALL_FRAMES, Player};

    use super::*;

    fn pump(audio: &mut crate::Audio, player: &mut Player) -> crate::Snapshot {
        let mut buf = vec![0i16; 2 * CALL_FRAMES];
        audio.render_i16(&mut buf);
        *player.snapshot()
    }

    #[test]
    fn start_sets_the_tempo_and_stop_leaves_the_position() {
        let sync = Arc::new(MidiSync::new());
        let (mut player, mut audio) = Player::new(Arc::new(Tables::new()), None);
        audio.set_midi_sync(Arc::clone(&sync));
        sync.set_tempo(100_000);
        sync.start();
        let first = pump(&mut audio, &mut player);
        assert!(first.playing);
        assert_eq!(first.tempo, 100_000);
        assert!(
            first.tick > 0,
            "the clock runs after Start, tick {}",
            first.tick
        );
        let second = pump(&mut audio, &mut player);
        assert!(second.tick > first.tick, "a repeated apply does not rewind");
        sync.stop();
        let stopped = pump(&mut audio, &mut player);
        assert!(!stopped.playing);
        assert_eq!(stopped.tick, second.tick);
        sync.locate(192);
        let moved = pump(&mut audio, &mut player);
        assert_eq!(moved.tick, 192);
        assert!(!moved.playing);
        sync.cont();
        let resumed = pump(&mut audio, &mut player);
        assert!(resumed.playing);
        assert!(resumed.tick >= 192);
    }
}
