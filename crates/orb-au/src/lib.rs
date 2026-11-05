//! The Audio Unit side of the iPad build. A host (AUM and the like) loads it on a
//! channel and processes its output. There is no view and no keyboard.
//!
//! The clock here follows the host. The application never calls [`Engine::follow_host`],
//! so a song opened as the application keeps the song's own tempo. The unit plays a short
//! TR-909 pattern on the built-in kit. The standard kit has no demo song, and a free Apple
//! ID cannot share the song open in the application with the plugin.

use std::sync::Arc;

use orb_engine::frame::{Devices, Engine, MAX_FRAMES};
use orb_engine::process::ProcessState;
use orb_engine::song::Device;
use orb_engine::tables::Tables;

/// Host frames rendered in one slice. The unit asks the host for no more than this.
const MAX_OUT: usize = 4096;
/// Engine frames kept for one slice. A 4096-frame buffer at 8 kHz is about 22600
/// frames at 44.1 kHz.
const MAX_ENGINE: usize = 24_000;

/// The engine and the buffers the render callback reuses.
pub struct Host {
    engine: Engine,
    pcm: Vec<i16>,
    eng_l: Vec<f32>,
    eng_r: Vec<f32>,
}

impl Host {
    /// An engine on the built-in kit, with a short TR-909 pattern in pattern 0.
    pub fn new() -> Self {
        let samples = rbm::standard::builtin_samples();
        let mut engine = Engine::new(
            Arc::new(Tables::new()),
            Devices::implemented_with(Some(samples), ProcessState::new()),
        );
        program_pattern(&mut engine);
        Self {
            engine,
            pcm: vec![0; MAX_FRAMES * 2],
            eng_l: vec![0.0; MAX_ENGINE],
            eng_r: vec![0.0; MAX_ENGINE],
        }
    }

    /// Fill `left` and `right` (one host frame each) from the engine.
    ///
    /// `tempo_bpm` and `beat` describe the host at the first frame. `playing` is the
    /// host transport. The engine always runs at 44.1 kHz; this resamples to `host_rate`.
    pub fn render(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        host_rate: f64,
        tempo_bpm: f64,
        beat: f64,
        playing: bool,
    ) {
        let n = left.len().min(right.len());
        let rate = if host_rate.is_finite() && host_rate >= 8_000.0 {
            host_rate
        } else {
            44_100.0
        };
        let mut done = 0;
        let mut beat = beat;
        while done < n {
            let k = (n - done).min(MAX_OUT);
            self.sync_clock(tempo_bpm, beat, playing);
            self.render_slice(&mut left[done..done + k], &mut right[done..done + k], rate);
            if playing && beat.is_finite() && tempo_bpm.is_finite() && tempo_bpm > 0.0 {
                beat += k as f64 * tempo_bpm / 60.0 / rate;
            }
            done += k;
        }
    }

    /// A finite `beat` locks the clock to the host. A host that is playing but has no beat
    /// (not a number) leaves the engine's own clock running, so the pattern is not pulled
    /// back to zero on every buffer. A stopped host halts.
    fn sync_clock(&mut self, tempo_bpm: f64, beat: f64, playing: bool) {
        if !playing {
            self.engine.follow_host(tempo_bpm, 0.0, false);
            return;
        }
        if beat.is_finite() {
            self.engine.follow_host(tempo_bpm, beat, true);
            return;
        }
        if tempo_bpm.is_finite() && (1.0..=999.0).contains(&tempo_bpm) {
            self.engine.set_tempo((tempo_bpm * 1000.0).round() as u32);
        }
        if !self.engine.playing() {
            self.engine.play();
        }
    }

    fn render_slice(&mut self, left: &mut [f32], right: &mut [f32], rate: f64) {
        let n = left.len();
        let need = ((n as f64) * 44_100.0 / rate).round() as usize;
        let need = need.clamp(1, MAX_ENGINE);
        let mut filled = 0;
        while filled < need {
            let chunk = (need - filled).min(MAX_FRAMES);
            let pcm = &mut self.pcm[..chunk * 2];
            pcm.fill(0);
            self.engine.render(pcm, chunk);
            for i in 0..chunk {
                self.eng_l[filled + i] = pcm[2 * i] as f32 / 32_768.0;
                self.eng_r[filled + i] = pcm[2 * i + 1] as f32 / 32_768.0;
            }
            filled += chunk;
        }
        stretch(&self.eng_l[..need], &self.eng_r[..need], left, right);
    }
}

impl Default for Host {
    fn default() -> Self {
        Self::new()
    }
}

/// Bass on the beat, snare on the backbeat, closed hats on the offbeats. Pattern 0,
/// which is the pattern a fresh engine plays.
fn program_pattern(engine: &mut Engine) {
    for step in [0, 4, 8, 12] {
        engine.step_field(Device::Tr909, 0, step, 1, 1);
    }
    for step in [4, 12] {
        engine.step_field(Device::Tr909, 0, step, 2, 1);
    }
    for step in [2, 6, 10, 14] {
        engine.step_field(Device::Tr909, 0, step, 8, 1);
    }
}

/// Linear map of `src` onto `dst`. Equal lengths are copied.
fn stretch(src_l: &[f32], src_r: &[f32], dst_l: &mut [f32], dst_r: &mut [f32]) {
    let ns = src_l.len().min(src_r.len());
    let nd = dst_l.len().min(dst_r.len());
    if ns == 0 || nd == 0 {
        return;
    }
    if ns == nd {
        dst_l[..nd].copy_from_slice(&src_l[..nd]);
        dst_r[..nd].copy_from_slice(&src_r[..nd]);
        return;
    }
    let step = ns as f64 / nd as f64;
    for i in 0..nd {
        let x = i as f64 * step;
        let i0 = (x as usize).min(ns - 1);
        let i1 = (i0 + 1).min(ns - 1);
        let frac = (x - i0 as f64) as f32;
        dst_l[i] = src_l[i0] * (1.0 - frac) + src_l[i1] * frac;
        dst_r[i] = src_r[i0] * (1.0 - frac) + src_r[i1] * frac;
    }
}

/// The host's engine. The audio callback is the only caller.
pub struct OrbAu {
    host: Host,
}

/// # Safety
/// Returns a unit the caller frees with [`orb_au_free`].
#[unsafe(no_mangle)]
pub extern "C" fn orb_au_new() -> *mut OrbAu {
    Box::into_raw(Box::new(OrbAu { host: Host::new() }))
}

/// # Safety
/// `au` is null or a pointer from [`orb_au_new`] not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn orb_au_free(au: *mut OrbAu) {
    if !au.is_null() {
        drop(unsafe { Box::from_raw(au) });
    }
}

/// # Safety
/// `left` and `right` each have `frames` floats. `au` is a live [`orb_au_new`] pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn orb_au_render(
    au: *mut OrbAu,
    left: *mut f32,
    right: *mut f32,
    frames: u32,
    host_rate: f64,
    tempo_bpm: f64,
    beat: f64,
    playing: i32,
) {
    if au.is_null() || left.is_null() || right.is_null() || frames == 0 {
        return;
    }
    let n = frames as usize;
    let left = unsafe { std::slice::from_raw_parts_mut(left, n) };
    let right = unsafe { std::slice::from_raw_parts_mut(right, n) };
    unsafe { &mut *au }
        .host
        .render(left, right, host_rate, tempo_bpm, beat, playing != 0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use orb_engine::frame::Devices;
    use orb_engine::seq::TEMPO_STEP;

    fn bare() -> Engine {
        Engine::new(Arc::new(Tables::new()), Devices::implemented(None))
    }

    #[test]
    fn the_host_clock_is_idle_until_the_host_plays() {
        let mut engine = bare();
        engine.follow_host(120.0, 1.0, false);
        assert!(!engine.playing());
        assert_eq!(engine.frame.seq.pos, 0.0);

        engine.follow_host(120.0, 1.0, true);
        assert!(engine.playing());
        assert!((engine.frame.seq.pos - 192.0).abs() < 1e-9);
        assert_eq!(engine.frame.seq.globals.tempo, 120_000);

        engine.follow_host(120.0, 1.25, true);
        assert!((engine.frame.seq.pos - 240.0).abs() < 1e-9);

        engine.follow_host(140.0, 8.0, true);
        assert!((engine.frame.seq.pos - 1_536.0).abs() < 1e-9);
        assert_eq!(engine.frame.seq.globals.tempo, 140_000);

        let held = engine.frame.seq.pos;
        engine.follow_host(140.0, 8.0, false);
        assert!(!engine.playing());
        assert_eq!(engine.frame.seq.pos, held);
    }

    #[test]
    fn a_running_host_does_not_restart_the_clock_every_buffer() {
        let mut engine = bare();
        engine.follow_host(120.0, 0.0, true);
        engine.render(&mut [0; 128], 64);
        let inc = 120_000.0 * TEMPO_STEP;
        assert!((engine.frame.seq.pos - inc).abs() < 1e-6);
        let tick = engine.frame.seq.last_tick;
        engine.follow_host(120.0, inc / 192.0, true);
        assert_eq!(engine.frame.seq.last_tick, tick);
        assert!((engine.frame.seq.pos - inc).abs() < 1e-6);
    }

    #[test]
    fn the_builtin_pattern_is_silent_until_the_host_plays() {
        let mut host = Host::new();
        let mut left = vec![0.0; 256];
        let mut right = vec![0.0; 256];
        host.render(&mut left, &mut right, 44_100.0, 120.0, 0.0, false);
        assert!(left.iter().all(|s| *s == 0.0));
        host.render(&mut left, &mut right, 44_100.0, 120.0, 0.0, true);
        assert!(host.engine.playing());
        assert!(left.iter().any(|s| s.abs() > 1.0e-4));
        assert!(host.engine.fault().is_none());
    }

    #[test]
    fn a_playing_host_without_a_beat_does_not_rewind() {
        let mut host = Host::new();
        let mut left = vec![0.0; 256];
        let mut right = vec![0.0; 256];
        host.render(&mut left, &mut right, 44_100.0, f64::NAN, f64::NAN, true);
        let pos = host.engine.frame.seq.pos;
        assert!(host.engine.playing());
        assert!(pos > 1.0);
        host.render(&mut left, &mut right, 44_100.0, f64::NAN, f64::NAN, true);
        assert!(host.engine.frame.seq.pos > pos);
    }
}
