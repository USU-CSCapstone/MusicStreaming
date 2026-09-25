//! Waveform bins over a mono downmix (`requirements/playback.md` §3).
//!
//! Stores peak and RMS per bin, quantized. Deterministic: the same samples in
//! the same order always give the same bytes. No legibility shaping here;
//! that happens at render time from these honest measurements.

use crate::types::Waveform;

/// Frames per fine-grained chunk when the total length is unknown up front.
const CHUNK_FRAMES: usize = 1024;

pub struct WaveformBuilder {
    bins: usize,
    /// Frames per bin when the length was known; otherwise `CHUNK_FRAMES`.
    frames_per_slot: u64,
    peaks: Vec<f32>,
    sum_squares: Vec<f64>,
    counts: Vec<u64>,
    frames_seen: u64,
}

impl WaveformBuilder {
    pub fn new(bins: usize, estimated_frames: Option<u64>) -> WaveformBuilder {
        let bins = bins.max(1);
        let frames_per_slot = match estimated_frames {
            Some(n) if n > 0 => (n / bins as u64).max(1),
            _ => CHUNK_FRAMES as u64,
        };
        WaveformBuilder {
            bins,
            frames_per_slot,
            peaks: Vec::new(),
            sum_squares: Vec::new(),
            counts: Vec::new(),
            frames_seen: 0,
        }
    }

    pub fn push_mono(&mut self, mono: &[f32]) {
        for &s in mono {
            let slot = (self.frames_seen / self.frames_per_slot) as usize;
            if slot >= self.peaks.len() {
                self.peaks.resize(slot + 1, 0.0);
                self.sum_squares.resize(slot + 1, 0.0);
                self.counts.resize(slot + 1, 0);
            }
            let a = s.abs();
            if a > self.peaks[slot] {
                self.peaks[slot] = a;
            }
            self.sum_squares[slot] += (s as f64) * (s as f64);
            self.counts[slot] += 1;
            self.frames_seen += 1;
        }
    }

    /// Resample the slots to exactly `bins` and quantize.
    pub fn finish(self) -> Waveform {
        let slots = self.peaks.len();
        if slots == 0 {
            return Waveform {
                peaks: vec![0; self.bins],
                rms: vec![0; self.bins],
            };
        }
        let mut peaks = Vec::with_capacity(self.bins);
        let mut rms = Vec::with_capacity(self.bins);
        for b in 0..self.bins {
            let start = b * slots / self.bins;
            let end = ((b + 1) * slots / self.bins).max(start + 1).min(slots);
            let mut peak: f32 = 0.0;
            let mut ss = 0.0f64;
            let mut n = 0u64;
            for i in start..end {
                peak = peak.max(self.peaks[i]);
                ss += self.sum_squares[i];
                n += self.counts[i];
            }
            let r = if n > 0 { (ss / n as f64).sqrt() } else { 0.0 };
            peaks.push(quantize(peak as f64));
            rms.push(quantize(r));
        }
        Waveform { peaks, rms }
    }
}

fn quantize(v: f64) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_shaped() {
        let mut a = WaveformBuilder::new(4, Some(400));
        let mut b = WaveformBuilder::new(4, Some(400));
        let mut samples = Vec::new();
        for i in 0..400 {
            // Quiet first half, loud second half.
            let amp = if i < 200 { 0.2 } else { 0.8 };
            samples.push(if i % 2 == 0 { amp } else { -amp });
        }
        a.push_mono(&samples[..150]);
        a.push_mono(&samples[150..]);
        b.push_mono(&samples);
        let wa = a.finish();
        let wb = b.finish();
        assert_eq!(wa, wb);
        assert_eq!(wa.peaks.len(), 4);
        assert!(wa.peaks[0] < wa.peaks[3]);
        assert_eq!(wa.peaks[3], quantize(0.8));
        assert_eq!(wa.rms[3], quantize(0.8));
    }

    #[test]
    fn unknown_length_still_yields_bins() {
        let mut w = WaveformBuilder::new(8, None);
        w.push_mono(&vec![0.5; 10_000]);
        let out = w.finish();
        assert_eq!(out.peaks.len(), 8);
        assert!(out.peaks.iter().all(|p| *p == quantize(0.5)));
    }

    #[test]
    fn empty_input() {
        let out = WaveformBuilder::new(3, Some(0)).finish();
        assert_eq!(out.peaks, vec![0, 0, 0]);
    }
}
