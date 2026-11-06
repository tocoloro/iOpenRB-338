//! MIDI clock for the iPad: tempo, start and stop.
//!
//! Clock, Start, Stop and Continue are system messages. They have no channel.
//! Song Position Pointer is a 14-bit position in sixteenth notes. The parser
//! turns those bytes into posts on [`orb_player::midi::MidiSync`]. The iPad
//! backend delivers the bytes from CoreMIDI.

/// The port Options > MIDI Sync is following.
#[cfg(target_os = "ios")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiChoice {
    Off,
    /// The virtual port this app publishes for a host such as AUM.
    Virtual,
    /// An external source. `id` is the CoreMIDI unique id as a bit pattern.
    Source(u32),
}

#[cfg(target_os = "ios")]
impl MidiChoice {
    /// The `midi_sync` settings value. Off is empty, so the key is left out.
    pub fn stored_id(self) -> String {
        match self {
            Self::Off => String::new(),
            Self::Virtual => "virtual".to_string(),
            Self::Source(id) => id.to_string(),
        }
    }
}

/// The byte parser. It is compiled for the iPad and for this crate's tests.
#[cfg(any(test, target_os = "ios"))]
mod clock {
    use orb_player::midi::MidiSync;

    /// One MIDI clock is this many nanoseconds at 20 BPM, the slowest interval that
    /// still counts. A longer gap is a pause, not a tempo.
    const SLOWEST_CLOCK_NS: u64 = 125_000_000;
    /// One clock at 400 BPM. Anything shorter is a burst, not a tempo.
    const FASTEST_CLOCK_NS: u64 = 6_250_000;
    /// BPM times 1000, from a one-clock interval in nanoseconds: `2.5e12 / dt`.
    const TEMPO_NUMERATOR: u128 = 2_500_000_000_000;
    /// How many clock intervals are averaged. 24 clocks are one quarter note.
    const INTERVALS: usize = 24;
    /// Intervals required before a tempo is posted, so two clocks cannot jump it.
    const INTERVALS_NEEDED: u8 = 3;
    /// Ticks in one MIDI beat (a sixteenth note). A quarter note is 192 ticks.
    const TICKS_PER_MIDI_BEAT: u32 = 48;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Position {
        Idle,
        Lsb,
        Msb(u8),
    }

    /// What one accepted byte stream produced.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Msg {
        Tempo(u32),
        Start,
        Stop,
        Continue,
        /// Song position in ticks.
        Position(u32),
    }

    /// Byte state for one MIDI source. Real-time bytes are taken wherever they sit,
    /// including between the two data bytes of a song position.
    #[derive(Clone, Debug)]
    pub(super) struct Parser {
        position: Position,
        last_ns: Option<u64>,
        intervals: [u64; INTERVALS],
        filled: u8,
        at: u8,
    }

    impl Parser {
        pub(super) fn new() -> Self {
            Self {
                position: Position::Idle,
                last_ns: None,
                intervals: [0; INTERVALS],
                filled: 0,
                at: 0,
            }
        }

        pub(super) fn reset(&mut self) {
            *self = Self::new();
        }

        /// Parses `bytes` and posts whatever they mean onto `sync`.
        pub(super) fn feed(&mut self, bytes: &[u8], time_ns: u64, sync: &MidiSync) {
            self.push(bytes, time_ns, |msg| post(sync, msg));
        }

        fn push(&mut self, bytes: &[u8], time_ns: u64, mut emit: impl FnMut(Msg)) {
            for &b in bytes {
                self.byte(b, time_ns, &mut emit);
            }
        }

        fn byte(&mut self, b: u8, time_ns: u64, emit: &mut impl FnMut(Msg)) {
            // Real-time bytes may sit inside any other message and do not cancel it.
            if (0xF8..=0xFF).contains(&b) {
                match b {
                    0xF8 => self.clock(time_ns, emit),
                    0xFA => emit(Msg::Start),
                    0xFB => emit(Msg::Continue),
                    0xFC => emit(Msg::Stop),
                    _ => {}
                }
                return;
            }
            match self.position {
                Position::Lsb if b < 0x80 => {
                    self.position = Position::Msb(b);
                    return;
                }
                Position::Msb(lsb) if b < 0x80 => {
                    self.position = Position::Idle;
                    let beats = u32::from(lsb) | (u32::from(b) << 7);
                    emit(Msg::Position(beats * TICKS_PER_MIDI_BEAT));
                    return;
                }
                Position::Lsb | Position::Msb(_) => self.position = Position::Idle,
                Position::Idle => {}
            }
            if b == 0xF2 {
                self.position = Position::Lsb;
            }
        }

        fn clock(&mut self, time_ns: u64, emit: &mut impl FnMut(Msg)) {
            let Some(prev) = self.last_ns else {
                self.last_ns = Some(time_ns);
                return;
            };
            self.last_ns = Some(time_ns);
            let dt = time_ns.saturating_sub(prev);
            if dt == 0 {
                return;
            }
            if !(FASTEST_CLOCK_NS..=SLOWEST_CLOCK_NS).contains(&dt) {
                self.filled = 0;
                self.at = 0;
                return;
            }
            self.intervals[self.at as usize] = dt;
            self.at = (self.at + 1) % INTERVALS as u8;
            if self.filled < INTERVALS as u8 {
                self.filled += 1;
            }
            if self.filled >= INTERVALS_NEEDED {
                emit(Msg::Tempo(tempo_of(
                    &self.intervals[..self.filled as usize],
                )));
            }
        }
    }

    fn tempo_of(intervals: &[u64]) -> u32 {
        let sum: u128 = intervals.iter().map(|dt| u128::from(*dt)).sum();
        let mean = sum / intervals.len() as u128;
        if mean == 0 {
            return 0;
        }
        let milli = (TEMPO_NUMERATOR + mean / 2) / mean;
        milli.min(999_000) as u32
    }

    fn post(sync: &MidiSync, msg: Msg) {
        match msg {
            Msg::Tempo(milli) => sync.set_tempo(milli),
            Msg::Start => sync.start(),
            Msg::Stop => sync.stop(),
            Msg::Continue => sync.cont(),
            Msg::Position(ticks) => sync.locate(ticks),
        }
    }

    /// The packet after `pkt` on ARM. `MIDIPacketNext` is a macro in the
    /// header, and the symbol is not exported, so the iPad callback cannot
    /// call it. The next packet begins at `data[length]`, rounded up to 4 bytes.
    pub(super) fn next_arm_packet(pkt: *const u8, length: u16) -> *const u8 {
        let end = pkt.wrapping_byte_add(10 + usize::from(length));
        let aligned = (end as usize).wrapping_add(3) & !3;
        aligned as *const u8
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn transport_and_song_position() {
            let mut parser = Parser::new();
            let mut out = Vec::new();
            // A note-on is not a clock message and does not eat the Start after it.
            parser.push(&[0x90, 0x3C, 0x7F, 0xFA, 0xFC, 0xFB], 0, |m| out.push(m));
            assert_eq!(out, vec![Msg::Start, Msg::Stop, Msg::Continue]);
            out.clear();
            // 4 sixteenth notes are one quarter note, 192 ticks.
            parser.push(&[0xF2, 0x04, 0x00], 0, |m| out.push(m));
            assert_eq!(out, vec![Msg::Position(192)]);
        }

        #[test]
        fn a_clock_between_the_position_bytes_is_a_clock() {
            let mut parser = Parser::new();
            let mut out = Vec::new();
            parser.push(&[0xF2, 0xF8, 0x04, 0x00], 1_000, |m| out.push(m));
            assert_eq!(out, vec![Msg::Position(192)]);
        }

        #[test]
        fn twenty_four_clocks_at_120_bpm_settle_on_120() {
            let mut parser = Parser::new();
            let mut tempos = Vec::new();
            let step = 20_833_333;
            for i in 0..24 {
                parser.push(&[0xF8], i * step, |m| tempos.push(m));
            }
            assert!(tempos.len() >= 20, "{tempos:?}");
            assert!(
                tempos.iter().all(|m| *m == Msg::Tempo(120_000)),
                "{tempos:?}"
            );
        }

        #[test]
        fn a_gap_does_not_become_the_tempo() {
            let mut parser = Parser::new();
            let mut tempos = Vec::new();
            parser.push(&[0xF8], 0, |m| tempos.push(m));
            parser.push(&[0xF8], 1_000_000_000, |m| tempos.push(m));
            let step = 20_833_333;
            for i in 0..4 {
                parser.push(&[0xF8], 1_000_000_000 + (i + 1) * step, |m| tempos.push(m));
            }
            assert_eq!(tempos, vec![Msg::Tempo(120_000), Msg::Tempo(120_000)]);
        }

        #[test]
        fn reset_drops_a_half_read_position_and_post_sets_the_tempo() {
            let mut parser = Parser::new();
            parser.push(&[0xF2, 0x04], 0, |_| {});
            parser.reset();
            let mut out = Vec::new();
            parser.push(&[0x00], 0, |m| out.push(m));
            assert!(out.is_empty());
            let sync = MidiSync::new();
            let step = 20_833_333;
            for i in 0..4 {
                parser.feed(&[0xF8], i * step, &sync);
            }
            assert_eq!(sync.tempo(), 120_000);
        }

        #[test]
        fn the_next_arm_packet_is_four_byte_aligned() {
            // A 4-byte-aligned packet. The header is 10 bytes, then the data.
            let pkt = 0x1000 as *const u8;
            let delta = |n: u16| super::next_arm_packet(pkt, n) as usize - 0x1000;
            assert_eq!(delta(0), 12);
            assert_eq!(delta(1), 12);
            assert_eq!(delta(2), 12);
            assert_eq!(delta(3), 16);
            assert_eq!(delta(4), 16);
            assert_eq!(super::next_arm_packet(pkt, 1) as usize % 4, 0);
        }
    }
}

#[cfg(target_os = "ios")]
#[path = "midi_ios.rs"]
mod ios;
#[cfg(target_os = "ios")]
pub use ios::{MidiIn, VIRTUAL_NAME};
