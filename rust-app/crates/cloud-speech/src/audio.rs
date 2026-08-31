//! Encoding recorded audio for upload.
//!
//! Every service in the catalogue accepts 16-bit PCM in a WAV container, and
//! several accept nothing else without extra negotiation. Dictation clips are
//! seconds long, so at 16 kHz mono a clip costs tens of kilobytes and there is
//! nothing to gain from a lossy codec and a decoder dependency.

/// The rate the speech models expect. Sending anything else makes the service
/// resample it, which costs quality and time.
pub const UPLOAD_SAMPLE_RATE_HZ: u32 = 16_000;

/// Writes mono 16-bit PCM samples into a WAV container.
///
/// Samples outside -1.0..=1.0 are clamped rather than allowed to wrap: a wrap
/// turns a loud syllable into a burst of noise, which a speech model will
/// happily transcribe as words that were never said.
#[must_use]
pub fn encode_wav(samples: &[f32], sample_rate_hz: u32) -> Vec<u8> {
    const HEADER_BYTES: usize = 44;
    let data_bytes = samples.len() * 2;
    let mut wav = Vec::with_capacity(HEADER_BYTES + data_bytes);

    let byte_rate = sample_rate_hz * 2;
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&((36 + data_bytes) as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVE");

    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes()); // PCM chunk size
    wav.extend_from_slice(&1_u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1_u16.to_le_bytes()); // mono
    wav.extend_from_slice(&sample_rate_hz.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes()); // block align
    wav.extend_from_slice(&16_u16.to_le_bytes()); // bits per sample

    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data_bytes as u32).to_le_bytes());
    for sample in samples {
        let clamped = sample.clamp(-1.0, 1.0);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the value is clamped to the i16 range immediately above"
        )]
        let encoded = (clamped * f32::from(i16::MAX)) as i16;
        wav.extend_from_slice(&encoded.to_le_bytes());
    }
    wav
}

#[cfg(test)]
mod tests {
    use super::{UPLOAD_SAMPLE_RATE_HZ, encode_wav};

    #[test]
    fn the_container_describes_mono_sixteen_bit_audio() {
        let wav = encode_wav(&[0.0; 8], UPLOAD_SAMPLE_RATE_HZ);

        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(u16::from_le_bytes([wav[22], wav[23]]), 1, "one channel");
        assert_eq!(
            u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]),
            UPLOAD_SAMPLE_RATE_HZ
        );
        assert_eq!(
            u16::from_le_bytes([wav[34], wav[35]]),
            16,
            "bits per sample"
        );
        assert_eq!(&wav[36..40], b"data");
    }

    #[test]
    fn the_declared_sizes_match_what_was_written() {
        let wav = encode_wav(&[0.5; 100], UPLOAD_SAMPLE_RATE_HZ);

        let riff_size = u32::from_le_bytes([wav[4], wav[5], wav[6], wav[7]]) as usize;
        let data_size = u32::from_le_bytes([wav[40], wav[41], wav[42], wav[43]]) as usize;

        assert_eq!(wav.len(), 44 + 200);
        assert_eq!(riff_size, wav.len() - 8);
        assert_eq!(data_size, 200);
    }

    #[test]
    fn a_sample_beyond_full_scale_is_clamped_rather_than_wrapped() {
        let wav = encode_wav(&[2.0, -2.0], UPLOAD_SAMPLE_RATE_HZ);

        assert_eq!(i16::from_le_bytes([wav[44], wav[45]]), i16::MAX);
        assert_eq!(i16::from_le_bytes([wav[46], wav[47]]), -i16::MAX);
    }

    #[test]
    fn silence_encodes_to_zeroes_and_an_empty_clip_to_a_bare_header() {
        assert_eq!(encode_wav(&[0.0, 0.0], UPLOAD_SAMPLE_RATE_HZ)[44..], [0; 4]);
        assert_eq!(encode_wav(&[], UPLOAD_SAMPLE_RATE_HZ).len(), 44);
    }
}
