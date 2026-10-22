//! The waveform's level scale and its shaping for the seek bar (`requirements/playback.md` §3).
//!
//! The scanner stores each bin's RMS as a level: a byte counting half decibels up from 127.5 dB
//! below full scale, so 255 is full scale and 0 is silence. Shaping turns a track's levels into
//! the heights a client draws, in integer math, so the same levels give the same heights on every
//! machine.

/// Level steps per decibel.
const STEPS_PER_DB: u8 = 2;

/// The height, of 255, at which a track's quietest sounding bins draw, so a quiet passage stays
/// visible.
const FLOOR: u32 = 24;

/// The narrowest range of levels shaping spreads over the full height, 6 dB, so a track of nearly
/// constant loudness does not draw its small differences as large ones.
const MIN_SPAN: u8 = 6 * STEPS_PER_DB;

/// The percentiles of a track's sounding bins that shaping draws at [`FLOOR`] and at full
/// height, so a few outlying bins do not set the scale for the rest.
const LOW_PERCENTILE: usize = 5;
const HIGH_PERCENTILE: usize = 95;

/// The level of `amplitude`, where 1.0 is full scale: 255 at full scale or above, one lower for
/// each half decibel below it, and 0 for silence and anything quieter than about 127 dB below
/// full scale.
pub fn waveform_level(amplitude: f64) -> u8 {
    let steps = 255.0 + f64::from(20 * STEPS_PER_DB) * amplitude.log10();
    // Silence is minus infinity, which the clamp makes 0; NaN, from a negative amplitude, casts
    // to 0.
    steps.round().clamp(0.0, 255.0) as u8
}

/// The heights, 0 to 255, a client draws for bins with the RMS levels `rms`, one per bin.
///
/// A silent bin draws 0. Every other bin is placed linearly in decibels between the 5th and 95th
/// percentiles of the track's own sounding bins, taken at least [`MIN_SPAN`] apart, from
/// [`FLOOR`] up to 255, and clamped to that range. A louder bin never draws lower than a quieter
/// one, so shaping moves no transition and invents no structure.
pub fn shape_waveform(rms: &[u8]) -> Vec<u8> {
    let mut sounding: Vec<u8> = rms.iter().copied().filter(|&level| level > 0).collect();
    sounding.sort_unstable();
    let Some(last) = sounding.len().checked_sub(1) else {
        return vec![0; rms.len()];
    };
    let high = sounding[last * HIGH_PERCENTILE / 100];
    let low = sounding[last * LOW_PERCENTILE / 100].min(high.saturating_sub(MIN_SPAN));
    // At least 1: `low` is `MIN_SPAN` or more below `high`, or 0 below a sounding `high`.
    let span = u32::from(high - low);
    rms.iter()
        .map(|&level| {
            if level == 0 {
                return 0;
            }
            let above = u32::from(level.clamp(low, high) - low);
            // Rounded to the nearest height; `above` is at most `span`, so this is at most 255.
            (FLOOR + (above * (255 - FLOOR) + span / 2) / span) as u8
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The level of a sound `db` decibels from full scale.
    fn level_at(db: f64) -> u8 {
        waveform_level(10f64.powf(db / 20.0))
    }

    #[test]
    fn levels_count_half_decibels_below_full_scale() {
        assert_eq!(waveform_level(1.0), 255);
        assert_eq!(waveform_level(2.0), 255);
        assert_eq!(level_at(-0.5), 254);
        assert_eq!(waveform_level(0.5), 243, "half scale is 6.02 dB down");
        assert_eq!(level_at(-120.0), 15);
        assert_eq!(level_at(-140.0), 0);
        assert_eq!(waveform_level(0.0), 0);
        assert_eq!(waveform_level(-0.5), 0);
        assert_eq!(waveform_level(f64::NAN), 0);
    }

    #[test]
    fn shaping_is_exact_integer_math() {
        assert_eq!(
            shape_waveform(&[0, 200, 210, 220, 230, 240, 250]),
            [0, 24, 82, 140, 197, 255, 255]
        );
    }

    #[test]
    fn a_quiet_and_a_loud_passage_both_stay_visible() {
        let passages = [level_at(-40.0), level_at(-20.0), level_at(-6.0)];
        let rms: Vec<u8> = passages.iter().flat_map(|&level| [level; 100]).collect();
        let shaped = shape_waveform(&rms);
        let (quiet, middle, loud) = (shaped[0], shaped[100], shaped[200]);
        assert_eq!((quiet, loud), (24, 255));
        assert!(quiet < middle && middle < loud, "{quiet} {middle} {loud}");
    }

    #[test]
    fn a_few_outlying_bins_do_not_set_the_scale() {
        let rms = [vec![50; 2], vec![200; 48], vec![230; 48], vec![255; 2]].concat();
        let shaped = shape_waveform(&rms);
        assert_eq!((shaped[0], shaped[2], shaped[50], shaped[98]), (24, 24, 255, 255));
    }

    #[test]
    fn a_compressed_master_is_no_solid_block() {
        let rms: Vec<u8> = [level_at(-9.0), level_at(-7.0)].repeat(50);
        let shaped = shape_waveform(&rms);
        assert_eq!(shaped[1], 255);
        assert!(shaped[1] - shaped[0] > 64, "verse {} chorus {}", shaped[0], shaped[1]);
    }

    #[test]
    fn nearly_constant_loudness_stays_nearly_flat() {
        let rms: Vec<u8> = [240, 241].repeat(50);
        assert_eq!(
            shape_waveform(&rms)[..2],
            [236, 255],
            "a one-step difference is drawn across a 6 dB span"
        );
    }

    #[test]
    fn the_floor_is_the_fifth_percentile() {
        // 40 levels, 100 to 139: the 5th percentile is 101 and the 95th is 137, so 100 and 101
        // both draw at the floor.
        let rms: Vec<u8> = (100..140).collect();
        assert_eq!(shape_waveform(&rms)[..4], [24, 24, 30, 37]);
    }

    #[test]
    fn a_louder_bin_never_draws_lower() {
        let rms: Vec<u8> = (0..512).map(|i| (i * 97 % 256) as u8).collect();
        let shaped = shape_waveform(&rms);
        let mut pairs: Vec<(u8, u8)> = rms.into_iter().zip(shaped).collect();
        pairs.sort();
        assert!(pairs.windows(2).all(|w| w[0].1 <= w[1].1));
    }

    #[test]
    fn silence_draws_nothing() {
        assert_eq!(shape_waveform(&[]), Vec::<u8>::new());
        assert_eq!(shape_waveform(&[0, 0, 0]), [0, 0, 0]);
        assert_eq!(shape_waveform(&[0, 1, 0]), [0, 255, 0]);
    }
}
