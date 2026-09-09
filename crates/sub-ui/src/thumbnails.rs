//! The texture cache behind the thumbnail strips painted on timeline clips
//! (docs/PLAN.md §5.7).
//!
//! The pictures themselves are made elsewhere: a
//! [`ThumbnailStrip`](sub_media::ThumbnailStrip) is the output of a thumbnail
//! job on the [`JobService`](sub_core::JobService), written once into the
//! project's sidecar directory. Nothing here decodes video, seeks or spawns a
//! job — the UI is handed strips through
//! [`ThumbnailCache::insert_strip`] and asks for the media it drew but has no
//! strip for through [`ThumbnailCache::take_missing`].
//!
//! What this module owns is the step between a JPEG on disk and a picture on
//! the timeline, and the three rules that keep that step off the frame budget:
//!
//! * **A bucket, not a size.** Textures are kept per [`ZoomBucket`] — a short
//!   ladder of sizes — rather than at whatever size this frame happens to
//!   want, so a smooth zoom or a resized panel re-uses textures instead of
//!   rebuilding them.
//! * **A budget.** The cache tracks the bytes its textures occupy and evicts
//!   whole strips, least recently used first, until it is back inside
//!   [`ThumbnailCacheConfig::budget_bytes`]. A strip drawn this frame is never
//!   the one evicted.
//! * **A ceiling on work per frame.** At most
//!   [`ThumbnailCacheConfig::uploads_per_frame`] textures are decoded and
//!   uploaded in any one painted frame. A tile whose texture is not resident
//!   yet is simply not painted — the clip shows its flat colour there — and
//!   it arrives a frame or two later. Scrolling past two hundred clips is
//!   therefore bounded work per frame however many tiles come into view.

use std::collections::{BTreeSet, HashMap};

use eframe::egui::{ColorImage, Context, TextureHandle, TextureOptions};
use sub_media::{ThumbnailImage, ThumbnailStrip};
use sub_model::MediaId;
use sub_time::{RationalTime, TimeRange};

/// The texture sizes a strip is kept at, largest dimension in pixels.
///
/// Four rungs an octave apart cover everything between a lane squeezed down
/// to a few points and a tall lane on a high-density display, and a zoom
/// gesture crosses at most one of them.
pub const BUCKET_SIZES: [u32; 4] = [32, 64, 128, 256];

/// How many bytes of thumbnail textures the timeline keeps by default.
///
/// Sixteen mebibytes is a few hundred tiles at the middle rungs: comfortably
/// more than one screen of a dense sequence, and small beside the frame cache.
pub const DEFAULT_BUDGET_BYTES: usize = 16 * 1024 * 1024;

/// How many textures are decoded and uploaded in one painted frame by
/// default.
pub const DEFAULT_UPLOADS_PER_FRAME: usize = 4;

/// The size a strip's textures are kept at.
///
/// The panel asks for the size it wants to draw a tile at — which the lane
/// height, the display's pixel density and, when a clip is narrower than one
/// whole tile, the zoom together decide — and the bucket rounds that up to a
/// rung of [`BUCKET_SIZES`]. Rounding up rather than down means a tile is
/// scaled down when it is painted, never up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ZoomBucket(u32);

impl ZoomBucket {
    /// The smallest rung of the ladder.
    pub const SMALLEST: Self = Self(BUCKET_SIZES[0]);

    /// The largest rung of the ladder.
    pub const LARGEST: Self = Self(BUCKET_SIZES[BUCKET_SIZES.len() - 1]);

    /// The rung that covers a tile `width` by `height` pixels.
    ///
    /// A tile larger than the largest rung is drawn from the largest rung: a
    /// thumbnail is an identification aid, and past a point a bigger texture
    /// costs memory without telling the editor anything more.
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the wanted size is clamped into the ladder before it is narrowed"
    )]
    pub fn for_tile(width: f32, height: f32) -> Self {
        let wanted = width.max(height);
        if !wanted.is_finite() || wanted <= 0.0 {
            return Self::SMALLEST;
        }
        let wanted = wanted.min(f32::from(u16::MAX)).ceil() as u32;
        for size in BUCKET_SIZES {
            if wanted <= size {
                return Self(size);
            }
        }
        Self::LARGEST
    }

    /// The largest dimension a texture in this bucket may have, in pixels.
    #[must_use]
    pub const fn size_px(self) -> u32 {
        self.0
    }
}

/// What the cache is allowed to spend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThumbnailCacheConfig {
    /// The bytes of texture the cache keeps before it starts evicting.
    pub budget_bytes: usize,
    /// The textures the cache decodes and uploads in one painted frame.
    pub uploads_per_frame: usize,
}

impl Default for ThumbnailCacheConfig {
    fn default() -> Self {
        Self {
            budget_bytes: DEFAULT_BUDGET_BYTES,
            uploads_per_frame: DEFAULT_UPLOADS_PER_FRAME,
        }
    }
}

/// What the cache has been doing, for the diagnostics panel and for tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ThumbnailCacheStats {
    /// Tiles served from a texture that was already resident.
    pub hits: usize,
    /// Tiles asked for whose texture was not resident.
    pub misses: usize,
    /// Textures decoded and uploaded.
    pub uploads: usize,
    /// Strips dropped to stay inside the budget.
    pub evictions: usize,
    /// Pictures that could not be read or decoded.
    pub decode_failures: usize,
}

/// Which strip, at which size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct StripKey {
    media: MediaId,
    bucket: ZoomBucket,
}

/// One strip's textures at one bucket.
struct StripEntry {
    /// One slot per picture of the strip; `None` until it is uploaded.
    textures: Vec<Option<TextureHandle>>,
    /// What the uploaded textures occupy.
    bytes: usize,
    /// The frame this entry was last drawn from.
    used_at: u64,
}

/// The timeline's thumbnail textures.
///
/// See the module documentation for the three rules it keeps.
pub struct ThumbnailCache {
    config: ThumbnailCacheConfig,
    /// The job output, by media item.
    strips: HashMap<MediaId, ThumbnailStrip>,
    /// The textures, by media item and bucket.
    entries: HashMap<StripKey, StripEntry>,
    /// Media the timeline drew but holds no strip for, for the caller to
    /// queue thumbnail jobs against.
    missing: BTreeSet<MediaId>,
    /// The painted frame counter, which is also the recency clock.
    clock: u64,
    /// Uploads left in this painted frame.
    uploads_left: usize,
    bytes: usize,
    stats: ThumbnailCacheStats,
}

impl Default for ThumbnailCache {
    fn default() -> Self {
        Self::new(ThumbnailCacheConfig::default())
    }
}

impl ThumbnailCache {
    /// An empty cache spending what `config` allows.
    #[must_use]
    pub fn new(config: ThumbnailCacheConfig) -> Self {
        Self {
            config,
            strips: HashMap::new(),
            entries: HashMap::new(),
            missing: BTreeSet::new(),
            clock: 0,
            uploads_left: config.uploads_per_frame,
            bytes: 0,
            stats: ThumbnailCacheStats::default(),
        }
    }

    /// What the cache is allowed to spend.
    #[must_use]
    pub const fn config(&self) -> ThumbnailCacheConfig {
        self.config
    }

    /// What the cache has been doing.
    #[must_use]
    pub const fn stats(&self) -> ThumbnailCacheStats {
        self.stats
    }

    /// The bytes the resident textures occupy.
    #[must_use]
    pub const fn bytes(&self) -> usize {
        self.bytes
    }

    /// How many (strip, bucket) pairs are resident.
    #[must_use]
    pub fn resident(&self) -> usize {
        self.entries.len()
    }

    /// Takes a generated strip, replacing whatever was known for that media.
    ///
    /// Replacing drops the textures built from the old strip: they were made
    /// from other pictures and would otherwise be painted for the new ones.
    pub fn insert_strip(&mut self, media: MediaId, strip: ThumbnailStrip) {
        self.forget_textures(media);
        self.missing.remove(&media);
        self.strips.insert(media, strip);
    }

    /// The strip known for `media`, if one has been inserted.
    #[must_use]
    pub fn strip(&self, media: MediaId) -> Option<&ThumbnailStrip> {
        self.strips.get(&media)
    }

    /// Drops everything known about `media`: its strip and its textures.
    pub fn forget(&mut self, media: MediaId) {
        self.forget_textures(media);
        self.strips.remove(&media);
        self.missing.remove(&media);
    }

    /// Starts a painted frame: a fresh upload allowance and a tick of the
    /// recency clock.
    ///
    /// The panel calls this once per frame before it paints any clip.
    pub const fn begin_frame(&mut self) {
        self.clock = self.clock.wrapping_add(1);
        self.uploads_left = self.config.uploads_per_frame;
    }

    /// The uploads left in this painted frame.
    #[must_use]
    pub const fn uploads_left(&self) -> usize {
        self.uploads_left
    }

    /// Notes that the timeline drew `media` and would show a strip for it.
    ///
    /// A media item with a strip already is not noted; everything else joins
    /// the set [`ThumbnailCache::take_missing`] hands back.
    pub fn want(&mut self, media: MediaId) {
        if !self.strips.contains_key(&media) {
            self.missing.insert(media);
        }
    }

    /// The media the timeline wanted a strip for and did not have, in id
    /// order, emptying the set.
    ///
    /// This is the seam the application turns into thumbnail jobs: the panel
    /// never spawns one itself.
    pub fn take_missing(&mut self) -> Vec<MediaId> {
        std::mem::take(&mut self.missing).into_iter().collect()
    }

    /// The media the timeline wanted a strip for and did not have.
    pub fn missing(&self) -> impl Iterator<Item = MediaId> + '_ {
        self.missing.iter().copied()
    }

    /// The texture for picture `index` of `media`'s strip at `bucket`.
    ///
    /// Returns `None` when there is no strip yet, when the picture cannot be
    /// decoded, or when this frame's upload allowance is spent — all of which
    /// the caller paints as "no picture here yet" rather than as an error.
    pub fn texture(
        &mut self,
        ctx: &Context,
        media: MediaId,
        bucket: ZoomBucket,
        index: usize,
    ) -> Option<TextureHandle> {
        let key = StripKey { media, bucket };
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.used_at = self.clock;
            if let Some(texture) = entry.textures.get(index).and_then(Option::as_ref) {
                self.stats.hits += 1;
                return Some(texture.clone());
            }
        }
        self.stats.misses += 1;
        if self.uploads_left == 0 {
            return None;
        }
        let strip = self.strips.get(&media)?;
        if index >= strip.frames().len() {
            return None;
        }
        let image = match strip.load_image(index) {
            Ok(image) => image,
            Err(err) => {
                // A picture that cannot be decoded still costs the frame an
                // upload: a strip whose files have been deleted underneath the
                // editor must not be able to spin on every tile of every
                // frame.
                self.uploads_left -= 1;
                self.stats.decode_failures += 1;
                log::debug!("thumbnail {index} of {media} cannot be drawn: {err}");
                return None;
            }
        };
        let frames = strip.frames().len();
        let color = scale_to_bucket(&image, bucket);
        let bytes = color.width() * color.height() * 4;
        let texture = ctx.load_texture(
            format!("timeline-thumb-{media}-{}-{index}", bucket.size_px()),
            color,
            TextureOptions::LINEAR,
        );

        self.uploads_left -= 1;
        self.stats.uploads += 1;
        let clock = self.clock;
        let entry = self.entries.entry(key).or_insert_with(|| StripEntry {
            textures: vec![None; frames],
            bytes: 0,
            used_at: clock,
        });
        entry.used_at = clock;
        entry.bytes += bytes;
        entry.textures[index] = Some(texture.clone());
        self.bytes += bytes;
        self.evict_to_budget();
        Some(texture)
    }

    /// Drops least recently used strips until the budget is met again.
    ///
    /// A strip drawn in this frame is never dropped: dropping it would only
    /// have it uploaded again next frame, which is thrashing rather than
    /// eviction.
    fn evict_to_budget(&mut self) {
        while self.bytes > self.config.budget_bytes {
            let victim = self
                .entries
                .iter()
                .filter(|(_, entry)| entry.used_at != self.clock)
                .min_by_key(|(_, entry)| entry.used_at)
                .map(|(key, _)| *key);
            let Some(victim) = victim else {
                return;
            };
            if let Some(entry) = self.entries.remove(&victim) {
                self.bytes = self.bytes.saturating_sub(entry.bytes);
                self.stats.evictions += 1;
            }
        }
    }

    /// Drops every texture built for `media`, at every bucket.
    fn forget_textures(&mut self, media: MediaId) {
        self.entries.retain(|key, entry| {
            let keep = key.media != media;
            if !keep {
                self.bytes = self.bytes.saturating_sub(entry.bytes);
            }
            keep
        });
    }
}

/// The source time the `index`-th of `count` tiles of a clip shows.
///
/// Each tile shows the middle of the slice of source it covers, which is
/// where [`strip_times`](sub_media::strip_times) puts the pictures themselves,
/// so a clip using its whole source lines its tiles up with the strip exactly.
/// The arithmetic is exact integer arithmetic in the clip's own rate: no part
/// of a timeline position is ever a float.
#[must_use]
pub fn tile_time(source_range: TimeRange, index: usize, count: usize) -> RationalTime {
    let rate = source_range.start().rate();
    if count == 0 {
        return source_range.start();
    }
    let start = i128::from(source_range.start().value());
    let duration = i128::from(source_range.duration().value());
    let index = i128::try_from(index).unwrap_or(0);
    let count = i128::try_from(count).unwrap_or(1).max(1);
    let offset = duration * (2 * index + 1) / (2 * count);
    let value = i64::try_from(start + offset).unwrap_or(source_range.start().value());
    RationalTime::new(value, rate)
}

/// The size a picture `width` by `height` is kept at in `bucket`: scaled down
/// to fit, never scaled up.
#[must_use]
pub fn fitted_size(width: u32, height: u32, bucket: ZoomBucket) -> (u32, u32) {
    let (width, height) = (width.max(1), height.max(1));
    let longest = width.max(height);
    let limit = bucket.size_px();
    if longest <= limit {
        return (width, height);
    }
    let scaled = |value: u32| {
        u32::try_from(u64::from(value) * u64::from(limit) / u64::from(longest))
            .unwrap_or(1)
            .max(1)
    };
    (scaled(width), scaled(height))
}

/// Scales a decoded thumbnail down into the bucket's size, as an egui image.
///
/// The downscale box-averages the source pixels each target pixel covers, the
/// same way the strip itself was written; point sampling a thumbnail of a
/// detailed shot aliases into noise that reads as a different shot.
#[must_use]
pub fn scale_to_bucket(image: &ThumbnailImage, bucket: ZoomBucket) -> ColorImage {
    let (target_width, target_height) = fitted_size(image.width, image.height, bucket);
    let source_width = image.width.max(1) as usize;
    let source_height = image.height.max(1) as usize;
    let (target_width, target_height) = (target_width as usize, target_height as usize);
    let mut rgba = vec![0xff_u8; target_width * target_height * 4];
    for ty in 0..target_height {
        let y0 = ty * source_height / target_height;
        let y1 = (((ty + 1) * source_height).div_ceil(target_height)).clamp(y0 + 1, source_height);
        for tx in 0..target_width {
            let x0 = tx * source_width / target_width;
            let x1 = (((tx + 1) * source_width).div_ceil(target_width)).clamp(x0 + 1, source_width);
            let mut sums = [0_u32; 3];
            let mut samples = 0_u32;
            for y in y0..y1 {
                for x in x0..x1 {
                    let offset = (y * source_width + x) * 4;
                    for (sum, channel) in sums.iter_mut().zip(0..3) {
                        *sum += u32::from(image.rgba.get(offset + channel).copied().unwrap_or(0));
                    }
                    samples += 1;
                }
            }
            let target = (ty * target_width + tx) * 4;
            for (channel, sum) in sums.into_iter().enumerate() {
                rgba[target + channel] = u8::try_from(sum / samples.max(1)).unwrap_or(u8::MAX);
            }
        }
    }
    ColorImage::from_rgba_unmultiplied([target_width, target_height], &rgba)
}

/// Strips made of written pictures, for the tests of this crate.
#[cfg(test)]
pub(crate) mod fixtures {
    use std::path::{Path, PathBuf};

    use sub_media::{ThumbnailFrame, ThumbnailOptions, ThumbnailStrip};
    use sub_model::ContentHash;

    /// A flat-colour JPEG of `width` by `height`, written to `path`.
    pub(crate) fn write_jpeg(path: &Path, width: u16, height: u16, colour: [u8; 3]) {
        let rgb: Vec<u8> = (0..u32::from(width) * u32::from(height))
            .flat_map(|_| colour)
            .collect();
        let mut encoded = Vec::new();
        jpeg_encoder::Encoder::new(&mut encoded, 90)
            .encode(&rgb, width, height, jpeg_encoder::ColorType::Rgb)
            .expect("an encoded thumbnail");
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("the directory");
        std::fs::write(path, encoded).expect("the picture");
    }

    /// A strip of `count` written pictures in a temporary directory.
    pub(crate) fn fixture_strip(dir: &Path, count: usize, size: (u16, u16)) -> ThumbnailStrip {
        let hash = ContentHash::from_bytes([3_u8; 32]);
        let options = ThumbnailOptions {
            count,
            ..ThumbnailOptions::default()
        };
        let frames = (0..count)
            .map(|index| {
                let file = ThumbnailStrip::frame_file_name(hash, options, index);
                write_jpeg(&dir.join(&file), size.0, size.1, [20, 120, 200]);
                ThumbnailFrame {
                    index,
                    pts_ns: i64::try_from(index).unwrap_or(0) * 1_000_000_000,
                    file,
                    width: u32::from(size.0),
                    height: u32::from(size.1),
                }
            })
            .collect();
        ThumbnailStrip::from_frames(dir, hash, options, frames).expect("a strip")
    }

    /// A fresh temporary directory named after the test using it.
    pub(crate) fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sub-ui-thumbs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }
}

#[cfg(test)]
mod tests {
    use sub_time::Rational;

    use super::fixtures::{fixture_strip, temp_dir};
    use super::*;

    #[test]
    fn the_bucket_ladder_rounds_a_wanted_tile_up() {
        assert_eq!(ZoomBucket::for_tile(1.0, 1.0), ZoomBucket::SMALLEST);
        assert_eq!(ZoomBucket::for_tile(32.0, 18.0).size_px(), 32);
        assert_eq!(ZoomBucket::for_tile(33.0, 18.0).size_px(), 64);
        assert_eq!(ZoomBucket::for_tile(18.0, 100.0).size_px(), 128);
        assert_eq!(ZoomBucket::for_tile(4000.0, 4000.0), ZoomBucket::LARGEST);
        // Degenerate sizes ask for the cheapest texture rather than panicking.
        assert_eq!(ZoomBucket::for_tile(0.0, 0.0), ZoomBucket::SMALLEST);
        assert_eq!(ZoomBucket::for_tile(f32::NAN, -3.0), ZoomBucket::SMALLEST);
        // The ladder is ordered, so a wider tile never asks for less texture.
        let mut previous = ZoomBucket::SMALLEST;
        for wanted in 1_u16..600 {
            let bucket = ZoomBucket::for_tile(f32::from(wanted), 0.0);
            assert!(bucket >= previous, "the ladder only climbs");
            previous = bucket;
        }
    }

    #[test]
    fn a_picture_is_scaled_into_its_bucket_and_never_up() {
        let image = ThumbnailImage {
            width: 320,
            height: 180,
            rgba: vec![0x40; 320 * 180 * 4],
        };
        assert_eq!(fitted_size(320, 180, ZoomBucket::SMALLEST), (32, 18));
        assert_eq!(fitted_size(320, 180, ZoomBucket::LARGEST), (256, 144));
        assert_eq!(fitted_size(16, 9, ZoomBucket::LARGEST), (16, 9));

        let scaled = scale_to_bucket(&image, ZoomBucket::SMALLEST);
        assert_eq!(scaled.size, [32, 18]);
        for pixel in &scaled.pixels {
            assert_eq!(
                (pixel.r(), pixel.g(), pixel.b(), pixel.a()),
                (64, 64, 64, 255)
            );
        }
    }

    #[test]
    fn tile_times_walk_the_source_range_in_exact_time() {
        let rate = Rational::FPS_24;
        let range = TimeRange::new(RationalTime::new(48, rate), RationalTime::new(96, rate))
            .expect("a range");
        let times: Vec<i64> = (0..4).map(|i| tile_time(range, i, 4).value()).collect();
        assert_eq!(times, vec![48 + 12, 48 + 36, 48 + 60, 48 + 84]);
        for index in 0..4 {
            let time = tile_time(range, index, 4);
            assert_eq!(time.rate(), rate);
            assert!(range.contains(time), "a tile shows its own clip");
        }
        // Degenerate counts fall back on the start rather than dividing by
        // zero.
        assert_eq!(tile_time(range, 0, 0), range.start());
    }

    #[test]
    fn textures_are_cached_per_bucket_and_uploaded_once() {
        let dir = temp_dir("cache");
        let strip = fixture_strip(&dir, 4, (64, 36));
        let ctx = Context::default();
        let media = MediaId::new();
        let mut cache = ThumbnailCache::default();
        cache.insert_strip(media, strip);

        cache.begin_frame();
        let first = cache
            .texture(&ctx, media, ZoomBucket::SMALLEST, 0)
            .expect("an uploaded texture");
        let again = cache
            .texture(&ctx, media, ZoomBucket::SMALLEST, 0)
            .expect("the same texture");
        assert_eq!(first.id(), again.id(), "a hit re-uses the texture");
        assert_eq!(cache.stats().uploads, 1);
        assert_eq!(cache.stats().hits, 1);

        // Another bucket is another texture, and both stay resident.
        let bigger = cache
            .texture(&ctx, media, ZoomBucket::LARGEST, 0)
            .expect("a second texture");
        assert_ne!(first.id(), bigger.id());
        assert_eq!(cache.resident(), 2);
        assert_eq!(cache.bytes(), 32 * 18 * 4 + 64 * 36 * 4);

        // A picture the strip does not hold is not an error, just no picture.
        assert!(
            cache
                .texture(&ctx, media, ZoomBucket::SMALLEST, 9)
                .is_none()
        );
        assert!(
            cache
                .texture(&ctx, MediaId::new(), ZoomBucket::SMALLEST, 0)
                .is_none()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn uploads_are_capped_per_painted_frame() {
        let dir = temp_dir("uploads");
        let strip = fixture_strip(&dir, 8, (64, 36));
        let ctx = Context::default();
        let media = MediaId::new();
        let mut cache = ThumbnailCache::new(ThumbnailCacheConfig {
            uploads_per_frame: 2,
            ..ThumbnailCacheConfig::default()
        });
        cache.insert_strip(media, strip);

        cache.begin_frame();
        let drawn = (0..8)
            .filter(|index| {
                cache
                    .texture(&ctx, media, ZoomBucket::SMALLEST, *index)
                    .is_some()
            })
            .count();
        assert_eq!(drawn, 2, "the frame budget bounds the work, not the tiles");
        assert_eq!(cache.uploads_left(), 0);

        cache.begin_frame();
        assert_eq!(cache.uploads_left(), 2);
        assert!(
            cache
                .texture(&ctx, media, ZoomBucket::SMALLEST, 0)
                .is_some(),
            "an already resident tile costs no upload"
        );
        assert!(
            cache
                .texture(&ctx, media, ZoomBucket::SMALLEST, 2)
                .is_some()
        );
        assert!(
            cache
                .texture(&ctx, media, ZoomBucket::SMALLEST, 3)
                .is_some()
        );
        assert!(
            cache
                .texture(&ctx, media, ZoomBucket::SMALLEST, 4)
                .is_none()
        );
        assert_eq!(cache.stats().uploads, 4);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn strips_are_evicted_least_recently_drawn_first_under_the_budget() {
        let dir = temp_dir("evict");
        let ctx = Context::default();
        let mut cache = ThumbnailCache::new(ThumbnailCacheConfig {
            // Room for two 32x18 textures, not three.
            budget_bytes: 32 * 18 * 4 * 2 + 1,
            uploads_per_frame: 8,
        });
        let media: Vec<MediaId> = (0..3).map(|_| MediaId::new()).collect();
        for (index, id) in media.iter().enumerate() {
            let strip = fixture_strip(&dir.join(index.to_string()), 1, (64, 36));
            cache.insert_strip(*id, strip);
        }

        for id in &media {
            cache.begin_frame();
            assert!(cache.texture(&ctx, *id, ZoomBucket::SMALLEST, 0).is_some());
        }
        assert!(cache.bytes() <= cache.config().budget_bytes);
        assert_eq!(cache.resident(), 2);
        assert_eq!(cache.stats().evictions, 1);
        assert!(
            cache.strip(media[0]).is_some(),
            "eviction drops textures, never the strip itself"
        );

        // The strip drawn in this very frame survives; the oldest one goes.
        cache.begin_frame();
        assert!(
            cache
                .texture(&ctx, media[0], ZoomBucket::SMALLEST, 0)
                .is_some()
        );
        assert_eq!(cache.resident(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn replacing_a_strip_drops_the_textures_made_from_the_old_one() {
        let dir = temp_dir("replace");
        let ctx = Context::default();
        let media = MediaId::new();
        let mut cache = ThumbnailCache::default();
        cache.insert_strip(media, fixture_strip(&dir.join("a"), 2, (64, 36)));
        cache.begin_frame();
        assert!(
            cache
                .texture(&ctx, media, ZoomBucket::SMALLEST, 0)
                .is_some()
        );
        assert_eq!(cache.resident(), 1);

        cache.insert_strip(media, fixture_strip(&dir.join("b"), 2, (64, 36)));
        assert_eq!(cache.resident(), 0);
        assert_eq!(cache.bytes(), 0);

        cache.forget(media);
        assert!(cache.strip(media).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn media_without_a_strip_is_reported_once_for_the_job_queue() {
        let dir = temp_dir("missing");
        let mut cache = ThumbnailCache::default();
        let (with, without) = (MediaId::new(), MediaId::new());
        cache.insert_strip(with, fixture_strip(&dir, 1, (64, 36)));

        cache.want(with);
        cache.want(without);
        cache.want(without);
        assert_eq!(cache.missing().collect::<Vec<_>>(), vec![without]);
        assert_eq!(cache.take_missing(), vec![without]);
        assert_eq!(cache.take_missing(), Vec::new());

        // A strip arriving clears the want, however it arrived.
        cache.want(without);
        cache.insert_strip(without, fixture_strip(&dir, 1, (64, 36)));
        assert_eq!(cache.take_missing(), Vec::new());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_picture_that_cannot_be_decoded_costs_an_upload_and_leaves_the_strip_alone() {
        let dir = temp_dir("corrupt");
        let strip = fixture_strip(&dir, 2, (64, 36));
        std::fs::write(strip.frames()[0].path(&dir), b"not a jpeg").expect("a corrupt picture");
        let ctx = Context::default();
        let media = MediaId::new();
        let mut cache = ThumbnailCache::default();
        cache.insert_strip(media, strip);

        cache.begin_frame();
        assert!(
            cache
                .texture(&ctx, media, ZoomBucket::SMALLEST, 0)
                .is_none()
        );
        assert_eq!(cache.stats().decode_failures, 1);
        assert_eq!(cache.bytes(), 0);
        assert_eq!(
            cache.uploads_left(),
            DEFAULT_UPLOADS_PER_FRAME - 1,
            "a failure costs the frame an upload rather than spinning"
        );
        // The rest of the strip still draws.
        assert!(
            cache
                .texture(&ctx, media, ZoomBucket::SMALLEST, 1)
                .is_some()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
