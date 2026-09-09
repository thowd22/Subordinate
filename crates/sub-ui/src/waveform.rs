//! Waveform strips on the timeline: the peaks `sub-media` generated, turned
//! into textures the panel draws audio clips with (docs/PLAN.md §5.7).
//!
//! A waveform is expensive to draw and cheap to reuse. One media item's peaks
//! become one texture — a picture of the whole source, not of one clip — and
//! every clip cut from that source draws a sub-rectangle of it, so trimming a
//! clip, scrolling the timeline or cutting a source into fifty pieces costs no
//! extra uploads. The cache holds those textures; nothing here decodes, reads
//! a file or blocks the UI thread.
//!
//! The peaks themselves come from
//! [`sub_media::Waveform`](sub_media::Waveform), whose pyramid holds several
//! zoom levels. [`level_for_zoom`] picks the one whose peaks land about one
//! per pixel at the timeline's current zoom, so a strip is never drawn from
//! more peaks than it has pixels to show them in.

use std::collections::HashMap;

use eframe::egui::{Color32, ColorImage, Context, Rect, TextureHandle, TextureOptions, pos2};
use sub_media::{Peak, Waveform, WaveformLevel};
use sub_model::MediaId;
use sub_time::{Rational, RationalTime, TimeRange};

use crate::timeline::ZoomLevel;

/// How tall one channel's band of a waveform texture is, in texels.
///
/// The texture is stretched to the clip's height when it is drawn, so this is
/// a resolution rather than a size: enough rows that a strip on a tall track
/// does not look blocky, few enough that a stereo texture stays small.
pub const CHANNEL_TEXELS: usize = 48;

/// The widest waveform texture that is ever built, in texels.
///
/// A three-hour source has millions of peaks at the finest level and no GPU
/// takes a texture that wide; the columns are merged down to this instead,
/// which is still far more detail than a timeline has pixels.
pub const MAX_TEXTURE_TEXELS: usize = 4_096;

/// The peaks of one media item at one zoom level, ready to be drawn.
///
/// Peaks are interleaved by channel exactly as
/// [`Waveform::read_peaks`](sub_media::Waveform::read_peaks) hands them over:
/// the peak of channel `c` in bucket `b` sits at `b * channels + c`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipWaveform {
    sample_rate: u32,
    channels: u16,
    frames_per_peak: u64,
    frames: u64,
    peaks: Vec<Peak>,
}

impl ClipWaveform {
    /// Builds a strip's worth of peaks directly, for tests and for callers
    /// that already hold the samples.
    ///
    /// The peak list is truncated to whole buckets: a trailing part-bucket
    /// would be drawn as a column of silence at the end of every clip.
    #[must_use]
    pub fn new(
        sample_rate: u32,
        channels: u16,
        frames_per_peak: u64,
        frames: u64,
        mut peaks: Vec<Peak>,
    ) -> Self {
        let channels = channels.max(1);
        let whole = peaks.len() - peaks.len() % usize::from(channels);
        peaks.truncate(whole);
        Self {
            sample_rate: sample_rate.max(1),
            channels,
            frames_per_peak: frames_per_peak.max(1),
            frames,
            peaks,
        }
    }

    /// Reads one level of a generated waveform.
    ///
    /// # Errors
    ///
    /// Returns `media.waveform_failed` when the level is not part of the
    /// waveform or its file cannot be read.
    pub fn from_level(waveform: &Waveform, level: &WaveformLevel) -> sub_core::SubResult<Self> {
        let peaks = waveform.read_peaks(level.index)?;
        Ok(Self::new(
            waveform.sample_rate(),
            waveform.channels(),
            level.frames_per_peak,
            waveform.frames(),
            peaks,
        ))
    }

    /// Sample rate of the source the peaks were taken from.
    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Channels the peaks interleave.
    #[must_use]
    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// Audio frames one peak summarises.
    #[must_use]
    pub fn frames_per_peak(&self) -> u64 {
        self.frames_per_peak
    }

    /// Audio frames the source carries.
    #[must_use]
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Buckets of peaks, one per channel each.
    #[must_use]
    pub fn buckets(&self) -> usize {
        self.peaks.len() / usize::from(self.channels)
    }

    /// The peaks, interleaved by channel.
    #[must_use]
    pub fn peaks(&self) -> &[Peak] {
        &self.peaks
    }

    /// Whether there is anything to draw at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.peaks.is_empty()
    }

    /// The part of this waveform a clip's source range covers, as texture
    /// coordinates in `0.0..=1.0`.
    ///
    /// A range past the end of the source — an unprobed clip, or one whose
    /// media was replaced by a shorter file — is clamped rather than allowed
    /// to sample outside the texture.
    #[must_use]
    pub fn uv_of(&self, source_range: TimeRange) -> Rect {
        let rate = Rational::new(self.sample_rate, 1).unwrap_or(Rational::FPS_25);
        let frames = self.frames.max(1);
        let position = |time: RationalTime| {
            let value = time.rescaled_to(rate).value().max(0);
            let clamped = u64::try_from(value).unwrap_or(0).min(frames);
            // One division at the end: the frame counts themselves stay exact.
            ratio(clamped, frames)
        };
        let left = position(source_range.start());
        let right = position(source_range.end_exclusive()).max(left);
        Rect::from_min_max(pos2(left, 0.0), pos2(right, 1.0))
    }
}

/// The textures the timeline draws waveforms with, one per media item.
///
/// The cache owns both the peaks it was handed and the texture built from
/// them. A texture is built once, on the first frame that draws the media, and
/// dropped when the peaks are replaced or the entry is removed — which is what
/// keeps painting a timeline full of audio clips proportional to what is on
/// screen rather than to how many clips there are.
#[derive(Default)]
pub struct WaveformCache {
    entries: HashMap<MediaId, Entry>,
}

/// One media item's peaks and the texture built from them.
struct Entry {
    waveform: ClipWaveform,
    texture: Option<TextureHandle>,
}

impl std::fmt::Debug for WaveformCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WaveformCache")
            .field("entries", &self.entries.len())
            .field("textures", &self.textures())
            .finish()
    }
}

impl WaveformCache {
    /// An empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Hands the cache the peaks of one media item, dropping any texture built
    /// from the peaks it replaces.
    pub fn insert(&mut self, media: MediaId, waveform: ClipWaveform) {
        self.entries.insert(
            media,
            Entry {
                waveform,
                texture: None,
            },
        );
    }

    /// Forgets one media item's peaks and its texture.
    pub fn remove(&mut self, media: MediaId) {
        self.entries.remove(&media);
    }

    /// Forgets everything, as a project close does.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// The peaks held for one media item, if any.
    #[must_use]
    pub fn waveform(&self, media: MediaId) -> Option<&ClipWaveform> {
        self.entries.get(&media).map(|entry| &entry.waveform)
    }

    /// How many media items the cache holds peaks for.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// How many textures have been built, which is how many media items have
    /// actually been drawn.
    #[must_use]
    pub fn textures(&self) -> usize {
        self.entries
            .values()
            .filter(|entry| entry.texture.is_some())
            .count()
    }

    /// Makes sure the texture for one media item exists, building it on the
    /// first call and reusing it on every later one.
    ///
    /// Returns whether there is a texture to draw with.
    pub fn prepare(&mut self, ctx: &Context, media: MediaId) -> bool {
        let Some(entry) = self.entries.get_mut(&media) else {
            return false;
        };
        if entry.texture.is_some() {
            return true;
        }
        if entry.waveform.is_empty() {
            return false;
        }
        let width = ctx
            .input(|input| input.max_texture_side)
            .min(MAX_TEXTURE_TEXELS);
        let image = waveform_image(&entry.waveform, width);
        entry.texture =
            Some(ctx.load_texture(format!("waveform-{media}"), image, TextureOptions::LINEAR));
        true
    }

    /// The texture for one media item, once [`WaveformCache::prepare`] has
    /// built it.
    #[must_use]
    pub fn texture(&self, media: MediaId) -> Option<&TextureHandle> {
        self.entries.get(&media)?.texture.as_ref()
    }
}

/// The level of a generated waveform to draw at `zoom`.
///
/// One timeline pixel covers a whole number of audio frames at the sequence
/// rate and the source's sample rate; the level whose peaks are no finer than
/// that is the one worth reading. The arithmetic is exact: the zoom is a
/// rational, the rate is a rational, and nothing here divides them as floats.
#[must_use]
pub fn level_for_zoom(
    waveform: &Waveform,
    zoom: ZoomLevel,
    rate: Rational,
) -> Option<&WaveformLevel> {
    waveform.level_for(frames_per_pixel(zoom, rate, waveform.sample_rate()))
}

/// Audio frames of a `sample_rate` source that one timeline pixel covers, at
/// `zoom` on a timeline whose timebase is `rate`.
///
/// At least one: a zoom so far in that a pixel is a fraction of an audio frame
/// still reads the finest peaks there are.
#[must_use]
pub fn frames_per_pixel(zoom: ZoomLevel, rate: Rational, sample_rate: u32) -> u64 {
    let pixels_per_frame = zoom.pixels_per_frame();
    // pixels per second = pixels_per_frame * rate, so
    // audio frames per pixel = sample_rate / (pixels_per_frame * rate).
    let numerator = u128::from(sample_rate)
        * u128::from(pixels_per_frame.denominator())
        * u128::from(rate.denominator());
    let denominator = u128::from(pixels_per_frame.numerator()) * u128::from(rate.numerator());
    u64::try_from(numerator / denominator.max(1))
        .unwrap_or(u64::MAX)
        .max(1)
}

/// Draws the peaks into an image: columns of time, one horizontal band per
/// channel.
///
/// The image is white and transparent, never coloured: it is drawn with a tint
/// so the same texture serves a selected clip, a locked one and a clip on a
/// muted track without being rebuilt.
///
/// Where the source has more buckets than the image has columns the extra ones
/// are merged rather than dropped, so a peak never disappears because the
/// picture was too small to show it.
#[must_use]
pub fn waveform_image(waveform: &ClipWaveform, max_width: usize) -> ColorImage {
    let channels = usize::from(waveform.channels());
    let buckets = waveform.buckets();
    let width = buckets.min(max_width.max(1)).max(1);
    let height = (channels * CHANNEL_TEXELS).max(1);
    let mut pixels = vec![Color32::TRANSPARENT; width * height];
    if buckets == 0 {
        return ColorImage::new([width, height], pixels);
    }
    for column in 0..width {
        // The buckets this column stands for, merged so nothing is lost.
        let first = column * buckets / width;
        let last = ((column + 1) * buckets / width).max(first + 1).min(buckets);
        for channel in 0..channels {
            let mut peak = Peak::SILENCE;
            for bucket in first..last {
                peak = peak.merged(waveform.peaks()[bucket * channels + channel]);
            }
            let band = channel * CHANNEL_TEXELS;
            let (top, bottom) = band_rows(peak);
            for row in top..=bottom {
                pixels[(band + row) * width + column] = Color32::WHITE;
            }
        }
    }
    ColorImage::new([width, height], pixels)
}

/// The first and last row of one channel's band that a peak fills.
///
/// A peak always fills at least one row: silence is a line down the middle of
/// the band, not a gap in the strip.
fn band_rows(peak: Peak) -> (usize, usize) {
    let centre = CHANNEL_TEXELS / 2;
    let rows = |value: f32, half: usize| -> usize {
        let scaled = value.abs().clamp(0.0, 1.0) * texels(half);
        // The clamp bounds the value to `half`, which a `usize` holds.
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the clamp above bounds the value to half the band"
        )]
        let rows = scaled.round() as usize;
        rows.min(half)
    };
    // A full-scale peak reaches the very top and the very bottom of its band;
    // the two halves differ by a row because the centre line belongs to both.
    let up = rows(peak.max_f32(), centre);
    let down = rows(peak.min_f32(), CHANNEL_TEXELS - centre - 1);
    (centre - up, centre + down)
}

/// `numerator / denominator` as a fraction, computed once from exact counts.
fn ratio(numerator: u64, denominator: u64) -> f32 {
    if denominator == 0 {
        return 0.0;
    }
    // The counts are frame positions, far inside the range an f64 holds
    // exactly, so the single division is the only rounding in the mapping.
    #[expect(
        clippy::cast_precision_loss,
        reason = "frame counts are exact in an f64 and the ratio is a screen coordinate"
    )]
    let ratio = numerator as f64 / denominator as f64;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a ratio in 0..=1 is exactly representable as f32"
    )]
    {
        ratio.clamp(0.0, 1.0) as f32
    }
}

/// A small texel count as a float, for scaling a peak into its band.
fn texels(count: usize) -> f32 {
    f32::from(u16::try_from(count).unwrap_or(u16::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stereo waveform whose left channel is full scale and whose right
    /// channel is half scale, `buckets` buckets long.
    fn stereo(buckets: usize) -> ClipWaveform {
        let peaks = (0..buckets)
            .flat_map(|_| {
                [
                    Peak {
                        min: -i16::MAX,
                        max: i16::MAX,
                    },
                    Peak {
                        min: -16_384,
                        max: 16_384,
                    },
                ]
            })
            .collect();
        ClipWaveform::new(48_000, 2, 512, (buckets as u64) * 512, peaks)
    }

    fn frames(value: i64) -> RationalTime {
        RationalTime::new(value, Rational::FPS_25)
    }

    #[test]
    fn a_part_bucket_at_the_end_is_never_half_drawn() {
        let waveform = ClipWaveform::new(48_000, 2, 512, 1_536, vec![Peak::SILENCE; 5]);
        assert_eq!(waveform.peaks().len(), 4, "the odd peak is dropped");
        assert_eq!(waveform.buckets(), 2);
    }

    #[test]
    fn the_uv_of_a_clip_is_its_source_range_over_the_whole_source() {
        // Ten seconds at 48 kHz, a clip covering the middle two seconds.
        let waveform = ClipWaveform::new(48_000, 2, 512, 480_000, vec![Peak::SILENCE; 1_876]);
        let range = TimeRange::new(frames(100), frames(50)).expect("a range");
        let uv = waveform.uv_of(range);
        assert!((uv.left() - 0.4).abs() < 1e-5, "four seconds in: {uv:?}");
        assert!((uv.right() - 0.6).abs() < 1e-5, "six seconds in: {uv:?}");
        assert!((uv.top() - 0.0).abs() < f32::EPSILON);
        assert!((uv.bottom() - 1.0).abs() < f32::EPSILON);

        // The whole source is the whole texture.
        let whole = waveform.uv_of(TimeRange::new(frames(0), frames(250)).expect("a range"));
        assert!((whole.left()).abs() < f32::EPSILON);
        assert!((whole.right() - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn a_range_past_the_end_of_the_source_is_clamped_into_the_texture() {
        let waveform = ClipWaveform::new(48_000, 1, 512, 48_000, vec![Peak::SILENCE; 94]);
        // Ten seconds of clip over a one-second source.
        let range = TimeRange::new(frames(0), frames(250)).expect("a range");
        let uv = waveform.uv_of(range);
        assert!(uv.left() >= 0.0 && uv.right() <= 1.0, "{uv:?}");
        assert!((uv.right() - 1.0).abs() < f32::EPSILON);
        // A negative start is a nonsense range, not a negative coordinate.
        let negative = TimeRange::new(frames(-50), frames(25)).expect("a range");
        assert!(waveform.uv_of(negative).left() >= 0.0);
    }

    #[test]
    fn one_pixel_covers_the_audio_frames_the_zoom_says_it_does() {
        let rate = Rational::FPS_25;
        // One pixel per video frame: a pixel is a 25th of a second, which at
        // 48 kHz is 1920 audio frames.
        let one = ZoomLevel::clamped(Rational::new(1, 1).expect("a zoom"));
        assert_eq!(frames_per_pixel(one, rate, 48_000), 1_920);
        // Ten pixels per frame shows ten times as much detail.
        let ten = ZoomLevel::clamped(Rational::new(10, 1).expect("a zoom"));
        assert_eq!(frames_per_pixel(ten, rate, 48_000), 192);
        // A frame per ten pixels, ten times less.
        let tenth = ZoomLevel::clamped(Rational::new(1, 10).expect("a zoom"));
        assert_eq!(frames_per_pixel(tenth, rate, 48_000), 19_200);
        // Never zero, however far in the timeline is zoomed.
        let far = ZoomLevel::clamped(Rational::new(4_096, 1).expect("a zoom"));
        assert!(frames_per_pixel(far, rate, 8_000) >= 1);
    }

    #[test]
    fn the_image_has_one_band_per_channel_and_draws_the_peaks_in_it() {
        let waveform = stereo(64);
        let image = waveform_image(&waveform, 128);
        assert_eq!(image.size, [64, 2 * CHANNEL_TEXELS]);

        let opaque = |x: usize, y: usize| image.pixels[y * image.size[0] + x].a() > 0;
        // The full-scale left channel reaches the top of its band; the
        // half-scale right channel does not reach the top of its.
        assert!(opaque(0, 0), "the left channel fills its band");
        assert!(
            !opaque(0, CHANNEL_TEXELS),
            "the right channel is half as tall"
        );
        // Both channels are drawn at their own centre line.
        assert!(opaque(0, CHANNEL_TEXELS / 2));
        assert!(opaque(0, CHANNEL_TEXELS + CHANNEL_TEXELS / 2));
    }

    #[test]
    fn silence_is_a_line_down_the_middle_rather_than_a_gap() {
        let waveform = ClipWaveform::new(48_000, 1, 512, 512, vec![Peak::SILENCE]);
        let image = waveform_image(&waveform, 64);
        assert_eq!(image.size, [1, CHANNEL_TEXELS]);
        let drawn = image.pixels.iter().filter(|pixel| pixel.a() > 0).count();
        assert_eq!(drawn, 1, "silence still draws its centre line");
    }

    #[test]
    fn a_source_wider_than_the_texture_merges_its_peaks_rather_than_losing_them() {
        // One loud bucket among a thousand quiet ones.
        let mut peaks = vec![
            Peak {
                min: -100,
                max: 100
            };
            1_000
        ];
        peaks[500] = Peak {
            min: -i16::MAX,
            max: i16::MAX,
        };
        let waveform = ClipWaveform::new(48_000, 1, 512, 512_000, peaks);
        let image = waveform_image(&waveform, 100);
        assert_eq!(image.size, [100, CHANNEL_TEXELS]);
        // The loud bucket is still there: some column reaches the top row.
        let top_row = &image.pixels[0..100];
        assert_eq!(
            top_row.iter().filter(|pixel| pixel.a() > 0).count(),
            1,
            "exactly the column holding the loud bucket reaches full scale"
        );
    }

    #[test]
    fn an_empty_waveform_makes_an_empty_picture_rather_than_a_panic() {
        let waveform = ClipWaveform::new(48_000, 2, 512, 0, Vec::new());
        assert!(waveform.is_empty());
        let image = waveform_image(&waveform, 64);
        assert_eq!(image.size, [1, 2 * CHANNEL_TEXELS]);
        assert!(image.pixels.iter().all(|pixel| pixel.a() == 0));
    }

    #[test]
    fn the_cache_builds_one_texture_per_media_item_and_reuses_it() {
        let ctx = Context::default();
        let mut cache = WaveformCache::new();
        let media = MediaId::new();
        assert!(cache.is_empty());
        assert!(!cache.prepare(&ctx, media), "nothing to draw yet");

        cache.insert(media, stereo(32));
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.textures(), 0, "a texture costs nothing until drawn");
        assert!(cache.prepare(&ctx, media));
        let first = cache.texture(media).expect("a texture").id();
        assert_eq!(cache.textures(), 1);

        // A second frame reuses the texture rather than uploading again.
        assert!(cache.prepare(&ctx, media));
        assert_eq!(cache.texture(media).expect("a texture").id(), first);

        // New peaks for the same media drop the old texture.
        cache.insert(media, stereo(64));
        assert_eq!(cache.textures(), 0);
        assert!(cache.prepare(&ctx, media));
        assert_eq!(cache.textures(), 1);

        cache.remove(media);
        assert!(cache.is_empty());
        assert!(cache.texture(media).is_none());
    }

    #[test]
    fn a_silent_media_item_never_costs_a_texture() {
        let ctx = Context::default();
        let mut cache = WaveformCache::new();
        let media = MediaId::new();
        cache.insert(media, ClipWaveform::new(48_000, 2, 512, 0, Vec::new()));
        assert!(!cache.prepare(&ctx, media), "there is nothing to upload");
        assert_eq!(cache.textures(), 0);
    }
}
