//! Waveform bins over a mono downmix (`requirements/playback.md` §3).
//!
//! Each bin is the RMS of its samples, as a level on the log scale of
//! [`jewelcase_core::waveform_level`]: the honest measurement, which the
//! server shapes for legibility with [`jewelcase_core::shape_waveform`].
//!
//! Deterministic: the same samples in the same order always give the same
//! bins.

use jewelcase_core::waveform_level;

/// Frames per fine-grained chunk when the total length is unknown up front.
const CHUNK_FRAMES: usize = 1024;

/// Sums the squares of a stream of mono samples per slot, then resamples the
/// slots to a fixed number of RMS levels.
pub struct WaveformBuilder {
    bins: usize,
    /// Frames per bin when the length was known; otherwise `CHUNK_FRAMES`.
    frames_per_slot: u64,
    sum_squares: Vec<f64>,
    counts: Vec<u64>,
    frames_seen: u64,
}

impl WaveformBuilder {
    /// A builder of `bins` levels, at least one. With `estimated_frames` each
    /// slot covers about one bin; without it slots are `CHUNK_FRAMES` long.
    pub fn new(bins: usize, estimated_frames: Option<u64>) -> WaveformBuilder {
        let bins = bins.max(1);
        let frames_per_slot = match estimated_frames {
            Some(n) if n > 0 => (n / bins as u64).max(1),
            _ => CHUNK_FRAMES as u64,
        };
        WaveformBuilder {
            bins,
            frames_per_slot,
            sum_squares: Vec::new(),
            counts: Vec::new(),
            frames_seen: 0,
        }
    }

    /// Adds the next samples of the downmix, in order.
    pub fn push_mono(&mut self, mono: &[f32]) {
        for &s in mono {
            let slot = (self.frames_seen / self.frames_per_slot) as usize;
            if slot >= self.counts.len() {
                self.sum_squares.resize(slot + 1, 0.0);
                self.counts.resize(slot + 1, 0);
            }
            self.sum_squares[slot] += (s as f64) * (s as f64);
            self.counts[slot] += 1;
            self.frames_seen += 1;
        }
    }

    /// Resamples the slots to exactly `bins` RMS levels, all silence when no
    /// samples came.
    pub fn finish(self) -> Vec<u8> {
        let slots = self.counts.len();
        if slots == 0 {
            return vec![0; self.bins];
        }
        let mut rms = Vec::with_capacity(self.bins);
        for b in 0..self.bins {
            let start = b * slots / self.bins;
            let end = ((b + 1) * slots / self.bins).max(start + 1).min(slots);
            let ss: f64 = self.sum_squares[start..end].iter().sum();
            let n: u64 = self.counts[start..end].iter().sum();
            let r = if n > 0 { (ss / n as f64).sqrt() } else { 0.0 };
            rms.push(waveform_level(r));
        }
        rms
    }
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
            // Quiet first half; loud second half, alternating 0.8 and -0.4.
            let sample = match (i < 200, i % 2 == 0) {
                (true, true) => 0.2,
                (true, false) => -0.2,
                (false, true) => 0.8,
                (false, false) => -0.4,
            };
            samples.push(sample);
        }
        a.push_mono(&samples[..150]);
        a.push_mono(&samples[150..]);
        b.push_mono(&samples);
        let wa = a.finish();
        let wb = b.finish();
        assert_eq!(wa, wb);
        assert_eq!(wa.len(), 4);
        assert_eq!(wa[0], waveform_level(0.2));
        // The RMS of 0.8 and 0.4 alternating is the square root of 0.4.
        assert_eq!(wa[3], waveform_level(0.4f64.sqrt()));
    }

    #[test]
    fn unknown_length_still_yields_bins() {
        let mut w = WaveformBuilder::new(8, None);
        w.push_mono(&vec![0.5; 10_000]);
        let out = w.finish();
        assert_eq!(out.len(), 8);
        assert!(out.iter().all(|r| *r == waveform_level(0.5)));
    }

    #[test]
    fn empty_input() {
        let out = WaveformBuilder::new(3, Some(0)).finish();
        assert_eq!(out, [0, 0, 0]);
    }
}
