//! The `track_waveforms.data` blob: a one-byte version, then one RMS level per bin, each a byte
//! on the scale of `jewelcase_core::waveform_level`.

/// The version byte this server writes and reads.
const VERSION: u8 = 2;

/// The blob for the RMS levels `rms`.
pub fn encode(rms: &[u8]) -> Vec<u8> {
    std::iter::once(VERSION).chain(rms.iter().copied()).collect()
}

/// The RMS levels in `blob`, or `None` when it does not start with this server's version byte.
pub fn decode(blob: &[u8]) -> Option<&[u8]> {
    let [VERSION, rms @ ..] = blob else {
        return None;
    };
    Some(rms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        assert_eq!(encode(&[4, 5, 6]), [2, 4, 5, 6]);
        assert_eq!(decode(&encode(&[4, 5, 6])), Some(&[4, 5, 6][..]));
        assert_eq!(decode(&encode(&[])), Some(&[][..]));
    }

    #[test]
    fn other_versions_are_refused() {
        assert_eq!(decode(&[]), None);
        assert_eq!(decode(&[1, 3, 0, 1, 2, 3, 4, 5, 6]), None, "peaks beside RMS");
        assert_eq!(decode(&[3, 4, 5, 6]), None);
    }
}
