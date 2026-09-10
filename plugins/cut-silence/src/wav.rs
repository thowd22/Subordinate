//! A small RIFF/WAVE reader: the sound the detector looks at.
//!
//! A sandboxed plugin has no decoder and no GStreamer; what it has is the
//! bytes of one file under the `$PROJECT` root the user approved. So this
//! reads the one container every editor's audio can be exported to and every
//! test fixture generator writes — linear PCM in a `.wav` — and nothing else.
//! A media item this cannot read is reported as such rather than guessed at:
//! see [`WavError`].
//!
//! Nothing here allocates per sample. [`Wav::frames`] borrows the file's bytes
//! and yields one mono `f32` per audio frame, so a long file costs one pass and
//! no second copy of itself.

/// How the samples in the data chunk are encoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleFormat {
    /// Signed little-endian integers, `bits` wide (8-bit PCM is unsigned and
    /// is not accepted).
    Int,
    /// IEEE 754 little-endian floats, 32 or 64 bits wide.
    Float,
}

/// Why a file is not sound this can read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WavError {
    /// The file does not begin `RIFF….WAVE`.
    NotWave,
    /// A chunk header or its payload runs past the end of the file.
    Truncated,
    /// There is no `fmt ` chunk, or it is too short to describe anything.
    MissingFormat,
    /// There is no `data` chunk.
    MissingData,
    /// The `fmt ` chunk describes an encoding this does not decode, such as
    /// A-law or ADPCM, carrying the WAVE format tag and the sample width.
    UnsupportedEncoding {
        /// The WAVE format tag: 1 is PCM, 3 is float, 0xfffe is extensible.
        format: u16,
        /// Bits per sample, as the header gives them.
        bits: u16,
    },
    /// The header claims no channels or no sample rate.
    EmptyHeader,
}

impl WavError {
    /// The one-line message this failure reaches a caller with.
    pub fn message(&self) -> String {
        match self {
            Self::NotWave => "the file is not a RIFF/WAVE file".to_owned(),
            Self::Truncated => "the file ends inside a chunk".to_owned(),
            Self::MissingFormat => "the file has no usable fmt chunk".to_owned(),
            Self::MissingData => "the file has no data chunk".to_owned(),
            Self::UnsupportedEncoding { format, bits } => format!(
                "the file is WAVE format {format} at {bits} bits, which this plugin does not \
                 decode; it reads 16, 24 and 32-bit integer PCM and 32 and 64-bit float PCM"
            ),
            Self::EmptyHeader => "the file claims no channels or no sample rate".to_owned(),
        }
    }
}

/// One decoded `.wav`, borrowing the file's bytes.
#[derive(Clone, Copy, Debug)]
pub struct Wav<'a> {
    /// Channels per frame; never zero.
    pub channels: u16,
    /// Frames per second; never zero. This is the analyzer's timebase: a
    /// silent span is reported as a tick count at exactly this rate.
    pub sample_rate: u32,
    /// Bits per sample.
    pub bits: u16,
    /// How a sample is encoded.
    pub format: SampleFormat,
    /// The data chunk, trimmed to a whole number of frames.
    data: &'a [u8],
}

/// The WAVE format tag for linear PCM.
const FORMAT_PCM: u16 = 1;
/// The WAVE format tag for IEEE float PCM.
const FORMAT_FLOAT: u16 = 3;
/// The WAVE format tag saying "the real tag is in the extension".
const FORMAT_EXTENSIBLE: u16 = 0xfffe;

impl<'a> Wav<'a> {
    /// Reads the header of `bytes` and finds its data chunk.
    ///
    /// # Errors
    ///
    /// A [`WavError`] describing what about the file cannot be read.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WavError> {
        if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
            return Err(WavError::NotWave);
        }

        let mut format: Option<(u16, u16, u32, u16)> = None;
        let mut data: Option<&[u8]> = None;
        let mut offset = 12;
        while offset + 8 <= bytes.len() {
            let id = &bytes[offset..offset + 4];
            let size = u32::from_le_bytes([
                bytes[offset + 4],
                bytes[offset + 5],
                bytes[offset + 6],
                bytes[offset + 7],
            ]) as usize;
            let start = offset + 8;
            let end = start.checked_add(size).ok_or(WavError::Truncated)?;
            if end > bytes.len() {
                return Err(WavError::Truncated);
            }
            match id {
                b"fmt " => format = Some(parse_format(&bytes[start..end])?),
                b"data" => data = Some(&bytes[start..end]),
                _ => {}
            }
            // Chunks are padded to an even length; the pad byte is not counted
            // in the size, so a file with an odd chunk misparses without this.
            offset = end + (end & 1);
        }

        let (tag, bits, sample_rate, channels) = format.ok_or(WavError::MissingFormat)?;
        let data = data.ok_or(WavError::MissingData)?;
        if channels == 0 || sample_rate == 0 {
            return Err(WavError::EmptyHeader);
        }
        let format = match (tag, bits) {
            (FORMAT_PCM, 16 | 24 | 32) => SampleFormat::Int,
            (FORMAT_FLOAT, 32 | 64) => SampleFormat::Float,
            _ => return Err(WavError::UnsupportedEncoding { format: tag, bits }),
        };

        let stride = usize::from(bits / 8) * usize::from(channels);
        let frames = data.len() / stride;
        Ok(Self {
            channels,
            sample_rate,
            bits,
            format,
            data: &data[..frames * stride],
        })
    }

    /// How many whole audio frames the data chunk holds.
    pub fn frame_count(&self) -> u64 {
        let stride = u64::from(self.bits / 8) * u64::from(self.channels);
        self.data.len() as u64 / stride
    }

    /// One mono sample per frame, the channels averaged.
    ///
    /// Averaging rather than taking the loudest channel is deliberate: a
    /// silence detector asks whether the *programme* is silent, and a stereo
    /// pair with one dead channel is not.
    pub fn frames(&self) -> Frames<'a> {
        Frames {
            data: self.data,
            offset: 0,
            channels: usize::from(self.channels),
            width: usize::from(self.bits / 8),
            format: self.format,
        }
    }
}

/// Reads the fields of a `fmt ` chunk: tag, bits, rate, channels.
fn parse_format(chunk: &[u8]) -> Result<(u16, u16, u32, u16), WavError> {
    if chunk.len() < 16 {
        return Err(WavError::MissingFormat);
    }
    let mut tag = u16::from_le_bytes([chunk[0], chunk[1]]);
    let channels = u16::from_le_bytes([chunk[2], chunk[3]]);
    let sample_rate = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
    let bits = u16::from_le_bytes([chunk[14], chunk[15]]);
    // WAVE_FORMAT_EXTENSIBLE keeps the real tag in the first two bytes of the
    // sub-format GUID, at the end of a 22-byte extension.
    if tag == FORMAT_EXTENSIBLE {
        if chunk.len() < 26 {
            return Err(WavError::MissingFormat);
        }
        tag = u16::from_le_bytes([chunk[24], chunk[25]]);
    }
    Ok((tag, bits, sample_rate, channels))
}

/// The mono samples of one file, one per audio frame.
#[derive(Clone, Debug)]
pub struct Frames<'a> {
    data: &'a [u8],
    offset: usize,
    channels: usize,
    width: usize,
    format: SampleFormat,
}

impl Iterator for Frames<'_> {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let stride = self.width * self.channels;
        if self.offset + stride > self.data.len() {
            return None;
        }
        let mut sum = 0.0_f64;
        for channel in 0..self.channels {
            let at = self.offset + channel * self.width;
            sum += f64::from(sample(&self.data[at..at + self.width], self.format));
        }
        self.offset += stride;
        // `channels` is never zero: `Wav::parse` rejects a header that says so.
        Some((sum / self.channels as f64) as f32)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let stride = self.width * self.channels;
        let left = (self.data.len() - self.offset) / stride;
        (left, Some(left))
    }
}

/// Decodes one sample to the -1..=1 range its width implies.
fn sample(bytes: &[u8], format: SampleFormat) -> f32 {
    match (format, bytes.len()) {
        (SampleFormat::Int, 2) => f32::from(i16::from_le_bytes([bytes[0], bytes[1]])) / 32_768.0,
        (SampleFormat::Int, 3) => {
            // Sign-extend 24 bits into the top three bytes of an i32.
            let value = i32::from_le_bytes([0, bytes[0], bytes[1], bytes[2]]) >> 8;
            value as f32 / 8_388_608.0
        }
        (SampleFormat::Int, 4) => {
            let value = i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            value as f32 / 2_147_483_648.0
        }
        (SampleFormat::Float, 4) => f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        (SampleFormat::Float, 8) => f64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]) as f32,
        // Unreachable: `Wav::parse` accepts no other width.
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::{SampleFormat, Wav, WavError};

    /// A `.wav` file holding `samples` as 16-bit stereo at 48 kHz.
    fn wav16(samples: &[i16], channels: u16) -> Vec<u8> {
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut fmt = Vec::new();
        fmt.extend(1_u16.to_le_bytes()); // PCM
        fmt.extend(channels.to_le_bytes());
        fmt.extend(48_000_u32.to_le_bytes());
        fmt.extend((48_000 * 2 * u32::from(channels)).to_le_bytes());
        fmt.extend((2 * channels).to_le_bytes());
        fmt.extend(16_u16.to_le_bytes());
        riff(&fmt, &data)
    }

    /// Wraps a `fmt ` payload and a `data` payload in a RIFF/WAVE container.
    fn riff(fmt: &[u8], data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend(b"RIFF");
        out.extend((4 + 8 + fmt.len() as u32 + 8 + data.len() as u32).to_le_bytes());
        out.extend(b"WAVE");
        out.extend(b"fmt ");
        out.extend((fmt.len() as u32).to_le_bytes());
        out.extend(fmt);
        out.extend(b"data");
        out.extend((data.len() as u32).to_le_bytes());
        out.extend(data);
        out
    }

    #[test]
    fn a_16_bit_stereo_file_reads_as_mono_frames_at_its_own_rate() {
        let bytes = wav16(&[0, 0, 32_767, -32_768, 16_384, 16_384], 2);
        let wav = Wav::parse(&bytes).expect("a wav");
        assert_eq!(wav.channels, 2);
        assert_eq!(wav.sample_rate, 48_000);
        assert_eq!(wav.format, SampleFormat::Int);
        assert_eq!(wav.frame_count(), 3);
        let frames: Vec<f32> = wav.frames().collect();
        assert_eq!(frames.len(), 3);
        assert!(frames[0].abs() < 1e-6);
        // The two channels cancel: a frame of +full and -full is not silence
        // to a peak meter but is to a downmix, which is what this measures.
        assert!(frames[1].abs() < 1e-3, "{}", frames[1]);
        assert!((frames[2] - 0.5).abs() < 1e-3, "{}", frames[2]);
    }

    #[test]
    fn a_float_file_and_a_24_bit_file_decode_to_the_same_level() {
        let mut fmt = Vec::new();
        fmt.extend(3_u16.to_le_bytes());
        fmt.extend(1_u16.to_le_bytes());
        fmt.extend(48_000_u32.to_le_bytes());
        fmt.extend((48_000_u32 * 4).to_le_bytes());
        fmt.extend(4_u16.to_le_bytes());
        fmt.extend(32_u16.to_le_bytes());
        let data: Vec<u8> = [0.5_f32, -0.25]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        let floats = riff(&fmt, &data);
        let wav = Wav::parse(&floats).expect("a float wav");
        assert_eq!(wav.format, SampleFormat::Float);
        let frames: Vec<f32> = wav.frames().collect();
        assert!((frames[0] - 0.5).abs() < 1e-6);
        assert!((frames[1] + 0.25).abs() < 1e-6);

        let mut fmt = Vec::new();
        fmt.extend(1_u16.to_le_bytes());
        fmt.extend(1_u16.to_le_bytes());
        fmt.extend(48_000_u32.to_le_bytes());
        fmt.extend((48_000_u32 * 3).to_le_bytes());
        fmt.extend(3_u16.to_le_bytes());
        fmt.extend(24_u16.to_le_bytes());
        let mut data = Vec::new();
        for value in [4_194_304_i32, -2_097_152] {
            data.extend(&value.to_le_bytes()[0..3]);
        }
        let ints = riff(&fmt, &data);
        let wav = Wav::parse(&ints).expect("a 24-bit wav");
        let frames: Vec<f32> = wav.frames().collect();
        assert!((frames[0] - 0.5).abs() < 1e-6, "{}", frames[0]);
        assert!((frames[1] + 0.25).abs() < 1e-6, "{}", frames[1]);
    }

    #[test]
    fn an_odd_length_chunk_before_the_data_chunk_is_stepped_over() {
        let mut bytes = wav16(&[1_000, 1_000], 1);
        // Splice a 3-byte LIST chunk, which carries a pad byte, in front of
        // `data`: the walk must land on `data` all the same.
        let at = bytes
            .windows(4)
            .position(|window| window == b"data")
            .expect("a data chunk");
        let mut chunk = Vec::new();
        chunk.extend(b"LIST");
        chunk.extend(3_u32.to_le_bytes());
        chunk.extend([1, 2, 3, 0]);
        bytes.splice(at..at, chunk);
        let wav = Wav::parse(&bytes).expect("a wav");
        assert_eq!(wav.frame_count(), 2);
    }

    #[test]
    fn what_cannot_be_read_says_so_rather_than_guessing() {
        assert_eq!(
            Wav::parse(b"not a wav at all").unwrap_err(),
            WavError::NotWave
        );

        let mut fmt = Vec::new();
        fmt.extend(6_u16.to_le_bytes()); // A-law
        fmt.extend(1_u16.to_le_bytes());
        fmt.extend(8_000_u32.to_le_bytes());
        fmt.extend(8_000_u32.to_le_bytes());
        fmt.extend(1_u16.to_le_bytes());
        fmt.extend(8_u16.to_le_bytes());
        let alaw = riff(&fmt, &[0, 0, 0, 0]);
        assert_eq!(
            Wav::parse(&alaw).unwrap_err(),
            WavError::UnsupportedEncoding { format: 6, bits: 8 }
        );

        let mut truncated = wav16(&[1, 2, 3, 4], 1);
        truncated.truncate(truncated.len() - 3);
        assert_eq!(Wav::parse(&truncated).unwrap_err(), WavError::Truncated);
        assert!(!WavError::NotWave.message().is_empty());
    }
}
