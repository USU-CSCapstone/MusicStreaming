//! The `track_waveforms.data` blob: a one-byte version, a two-byte little-endian bin count, then
//! that many peaks, then as many RMS values.

use jewelcase_scanner::Waveform;

/// The version byte this server writes and reads.
const VERSION: u8 = 1;

/// The blob for `waveform`. Its peaks and RMS are cut to the shorter of the two, and to at most
/// `u16::MAX` bins, so that [`decode`] always reads it back.
pub fn encode(waveform: &Waveform) -> Vec<u8> {
    let count = u16::try_from(waveform.peaks.len().min(waveform.rms.len())).unwrap_or(u16::MAX);
    let bins = usize::from(count);
    let mut out = Vec::with_capacity(3 + 2 * bins);
    out.push(VERSION);
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&waveform.peaks[..bins]);
    out.extend_from_slice(&waveform.rms[..bins]);
    out
}

/// The waveform in `blob`, or `None` when it has another version or is too short for its count.
pub fn decode(blob: &[u8]) -> Option<Waveform> {
    let [VERSION, low, high, rest @ ..] = blob else {
        return None;
    };
    let bins = usize::from(u16::from_le_bytes([*low, *high]));
    let (peaks, rest) = rest.split_at_checked(bins)?;
    let rms = rest.get(..bins)?;
    Some(Waveform { peaks: peaks.to_vec(), rms: rms.to_vec() })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A waveform with these bins.
    fn waveform(peaks: &[u8], rms: &[u8]) -> Waveform {
        Waveform { peaks: peaks.to_vec(), rms: rms.to_vec() }
    }

    #[test]
    fn round_trips() {
        let w = waveform(&[1, 2, 3], &[4, 5, 6]);
        assert_eq!(encode(&w), [1, 3, 0, 1, 2, 3, 4, 5, 6]);
        assert_eq!(decode(&encode(&w)), Some(w));
    }

    #[test]
    fn uneven_series_are_cut_to_the_shorter() {
        let short_rms = waveform(&[1, 2, 3], &[4, 5]);
        assert_eq!(decode(&encode(&short_rms)), Some(waveform(&[1, 2], &[4, 5])));
        let short_peaks = waveform(&[1, 2], &[4, 5, 6]);
        assert_eq!(decode(&encode(&short_peaks)), Some(waveform(&[1, 2], &[4, 5])));
    }

    #[test]
    fn bins_stop_at_the_count_limit() {
        let bins = usize::from(u16::MAX) + 1;
        let decoded = decode(&encode(&waveform(&vec![7; bins], &vec![8; bins]))).unwrap();
        assert_eq!((decoded.peaks.len(), decoded.rms.len()), (bins - 1, bins - 1));
    }

    #[test]
    fn other_versions_and_short_blobs_are_refused() {
        assert_eq!(decode(&[9, 0, 0]), None);
        assert_eq!(decode(&[1, 0]), None);
        assert_eq!(decode(&[1, 2, 0, 1, 2, 3]), None);
        assert_eq!(decode(&[1, 0, 0]), Some(waveform(&[], &[])));
    }
}
