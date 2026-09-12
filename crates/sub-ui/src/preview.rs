//! Decoded pictures for the viewer: one decode pipeline per active clip, off
//! the UI thread (docs/PLAN.md §4).
//!
//! The compositor has always sampled its layers through a frame source; until
//! now the editor handed it one that answered "no picture" for every clip, so
//! the viewer painted the bare black canvas however real the project was. This
//! is the other end of that interface.
//!
//! The shape is the one `sub_export::sequence::SequenceFrames` already uses to
//! render a sequence for export — resolve the layers under the playhead, seek
//! one decoder per clip to [`ResolvedClip::source_time`], upload the picture as
//! NV12 and hand the view to [`sub_render::Compositor::render`] — with the two
//! differences a live preview needs:
//!
//! * **Nothing decodes on the UI thread.** Each clip's decoder lives on a
//!   [`DecodeAhead`] worker of its own, and opening one (which builds a PTS
//!   index over the whole file) is a [`JobService`] job. The UI thread only ever
//!   *asks*: it posts a target, pops whatever the ring already holds, and paints
//!   the newest picture it has. A clip with nothing ready yet contributes no
//!   layer, exactly as a gap does, so a slow file costs a black frame and never
//!   a stalled window.
//! * **State is kept between frames.** A converter is built per clip and reused
//!   while the picture geometry holds, as the benchmark's uploader does, and the
//!   decoders stay open across playhead moves so a drag costs one step rather
//!   than one pipeline.
//!
//! Scrubbing and playing feed the same decoder differently, which is the point
//! of [`PreviewService::set_playing`]:
//!
//! * **Scrubbing** posts every target through [`DecodeAhead::seek_to`], which is
//!   [`sub_media::Decoder::seek_to`] on the worker with a [`PtsIndex`] set. That
//!   is the TASK-133 path: a step the decoder can reach by decoding on does not
//!   flush, a step back over ground the drag just covered is a GOP cache hit,
//!   and only a real jump seeks. It is also, stage for stage, what
//!   `subordinate-bench`'s `scrub_drag` scenario measures — seek, then the same
//!   NV12 upload.
//! * **Playing** lets the ring do its job: a forward step the decode-ahead can
//!   already reach is taken out of the buffer rather than seeked to, because a
//!   seek would throw the buffered pictures away and put the decoder back at a
//!   keyframe once a frame.
//!
//! Which file is read is the media item's own answer: [`MediaItem::source`] for
//! the viewer's [`MediaUse`], so a ready proxy is previewed when the Proxy
//! toggle is on (TASK-70) and never otherwise. Flipping the toggle drops the
//! open decoders, because the file behind them changed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use eframe::wgpu;
use sub_core::{JobContext, JobHandle, JobService, Priority, SubError, SubResult};
use sub_media::{
    DecodeAhead, Decoder, DecoderOptions, FrameFormat, LazyPtsIndex, PtsIndex, StreamSelection,
    VideoFrame,
};
use sub_model::{ClipId, MediaItem, MediaUse, Project, Sequence};
use sub_render::{
    Nv12Converter, Nv12Geometry, RenderContext, RenderError, ResolvedClip, SourceFrame,
    resolve_layers_at,
};
use sub_time::RationalTime;

/// The job kind opening a clip's decoder reports itself as.
pub const PREVIEW_JOB_KIND: &str = "preview-open";

/// How many clips keep a decode pipeline open at once.
///
/// One pipeline per active clip is the §4 rule, and "active" is a handful: the
/// layers under the playhead plus whatever a cut has just moved off. A bound is
/// still needed, because a playhead dragged across a long edit would otherwise
/// leave a GStreamer pipeline behind for every clip it passed.
pub const MAX_OPEN_CLIPS: usize = 8;

/// How many decoded pictures each clip's worker keeps ahead of the playhead.
///
/// [`sub_media::DEFAULT_CAPACITY`], named here because the same number decides
/// how far a forward step may reach into the ring before it is worth a seek.
pub const DECODE_AHEAD_FRAMES: usize = sub_media::DEFAULT_CAPACITY;

/// What the preview is doing, for the diagnostics panel and for tests.
///
/// The counters are cumulative over the life of the service; the three counts
/// are instantaneous.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PreviewStats {
    /// Clips with a decoder open right now.
    pub open: usize,
    /// Clips whose decoder is still being opened.
    pub opening: usize,
    /// Clips whose decoder could not be opened.
    pub failed: usize,
    /// Pictures delivered to the compositor.
    pub delivered: u64,
    /// Pictures the worker decoded that the playhead had already run past by
    /// the time they were popped, plus every picture a seek threw out of a
    /// ring. These are decoded and never shown: the preview's own dropped
    /// frames, counted separately from the presentations
    /// [`sub_edit::playback::PlaybackScheduler`] drops against the master
    /// clock.
    pub dropped: u64,
    /// Composites where the wanted picture was not ready and the layer was
    /// drawn with the newest picture there was, or with none at all.
    pub late: u64,
    /// How many layers the last composite had a decoded picture for.
    ///
    /// Zero is the bare black canvas, which is what the viewer drew for every
    /// project before this was wired up; one or more is decoded media on
    /// screen. The window's ready line reports it, so an unattended run can
    /// say whether the picture it photographed is real.
    pub showing: usize,
}

/// The pictures the compositor may sample this frame.
///
/// Handed out by [`PreviewService::pictures`] and consumed by a
/// [`sub_render::FrameSource`] closure over [`PreviewFrames::get`].
#[derive(Debug, Default)]
pub struct PreviewFrames {
    frames: HashMap<ClipId, SourceFrame>,
    changed: bool,
    busy: bool,
}

impl PreviewFrames {
    /// The picture for `clip`, or `None` when it has none ready.
    #[must_use]
    pub fn get(&self, clip: ClipId) -> Option<SourceFrame> {
        self.frames.get(&clip).cloned()
    }

    /// How many layers have a picture.
    #[must_use]
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Whether no layer has a picture, which composites to black.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Whether any layer's picture differs from the one the last call handed
    /// out, which is when a composite has to be redone even though the
    /// playhead has not moved.
    #[must_use]
    pub const fn changed(&self) -> bool {
        self.changed
    }

    /// Whether a decoder is still opening or a picture is still being waited
    /// for, which is when the window must repaint itself to pick it up.
    #[must_use]
    pub const fn busy(&self) -> bool {
        self.busy
    }
}

/// How a clip's decoder is moved to the picture that was asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Already showing it: nothing to do.
    Hold,
    /// The decode-ahead ring will deliver it; seeking would throw the ring
    /// away and put the decoder back at a keyframe.
    Ring,
    /// Post a seek, which is where the PTS index and the GOP cache work.
    Seek,
}

/// The picture of `index` a source time asks for.
///
/// The first picture at or after the time, which is exactly what
/// [`sub_media::Decoder::seek_to`] lands on — so the viewer shows the frame an
/// export of the same timeline would write, and a clip's source range means
/// the same thing in both. A time past the last picture holds on the last one
/// rather than going blank, because a clip trimmed a frame past its media's
/// end should show its last frame, not a hole.
fn picture_at(index: &PtsIndex, time: RationalTime) -> Option<usize> {
    if index.is_empty() {
        return None;
    }
    index
        .frame_at_or_after(time)
        .or_else(|| index.len().checked_sub(1))
}

/// Decides how to get from `current` to `wanted`.
///
/// The whole difference between scrubbing and playing is here. Scrubbing seeks
/// for every step, because that is the path the index and the GOP cache make
/// cheap and it is the path the benchmark measures. Playing takes a forward
/// step out of the ring whenever the ring can reach it, because a seek would
/// discard the decode-ahead that playback is living on.
const fn plan_step(
    current: Option<usize>,
    wanted: usize,
    playing: bool,
    eos: bool,
    capacity: usize,
) -> Step {
    if let Some(current) = current {
        if current == wanted {
            return Step::Hold;
        }
        if playing && !eos && wanted > current && wanted - current <= capacity {
            return Step::Ring;
        }
    }
    // Nothing delivered yet, a step backwards, a jump further than the ring
    // reaches, or a scrub: only a seek can aim the decoder at it.
    Step::Seek
}

/// A clip's decoder, opened on a worker and moved here once it is ready.
struct OpenedClip {
    ahead: DecodeAhead,
    index: Arc<PtsIndex>,
}

/// One clip's live decode state, on the UI thread.
struct ClipPreview {
    /// The worker holding this clip's pipeline.
    ahead: DecodeAhead,
    /// The index the worker's decoder plans its seeks from, used here to turn
    /// a source time into the picture that covers it.
    index: Arc<PtsIndex>,
    /// The picture asked for, as an index frame number.
    wanted: Option<usize>,
    /// The newest picture delivered, and which frame it is.
    current: Option<VideoFrame>,
    current_frame: Option<usize>,
    /// The frame sitting in the converter's texture, so an unchanged picture
    /// is not re-uploaded.
    uploaded: Option<usize>,
    /// The converter and the view of its output, rebuilt only when the
    /// picture geometry changes.
    converter: Option<Nv12Converter>,
    view: Option<wgpu::TextureView>,
    /// Whether the worker has reached the end of the file for this position.
    eos: bool,
    /// Pictures popped that the playhead had already passed.
    skipped: u64,
    /// Which pass over the layers last wanted this clip, for eviction.
    last_seen: u64,
}

impl ClipPreview {
    /// A preview over an opened clip.
    fn new(opened: OpenedClip, seen: u64) -> Self {
        Self {
            ahead: opened.ahead,
            index: opened.index,
            wanted: None,
            current: None,
            current_frame: None,
            uploaded: None,
            converter: None,
            view: None,
            eos: false,
            skipped: 0,
            last_seen: seen,
        }
    }

    /// Asks for the picture covering `source_time`.
    ///
    /// A target the ring can still reach is left to the ring while `playing`,
    /// because seeking would throw away the decode-ahead the playback is
    /// living on. Everything else — every scrub, and any jump — goes through
    /// the worker's [`sub_media::Decoder::seek_to`], which is where the PTS
    /// index and the GOP cache do their work.
    fn request(&mut self, source_time: RationalTime, playing: bool) -> SubResult<()> {
        let Some(frame) = picture_at(&self.index, source_time) else {
            // An empty index: there is nothing in the file to show, and the
            // layer keeps whatever it had rather than flashing.
            return Ok(());
        };
        if self.wanted == Some(frame) {
            return Ok(());
        }
        self.wanted = Some(frame);
        let step = plan_step(
            self.current_frame,
            frame,
            playing,
            self.eos,
            self.ahead.capacity(),
        );
        match step {
            Step::Hold | Step::Ring => Ok(()),
            Step::Seek => {
                let Some(pts) = self.index.pts(frame) else {
                    return Ok(());
                };
                self.eos = false;
                self.ahead.seek_to(pts)
            }
        }
    }

    /// Takes whatever the worker has ready, without ever waiting for it.
    ///
    /// Only ever pops while the ring is known to hold something, so this is
    /// the whole of the "the UI thread never blocks on decode" rule: a worker
    /// that is still seeking simply has an empty ring and this returns with
    /// the picture the viewer already had.
    fn poll(&mut self) -> SubResult<()> {
        let Some(wanted) = self.wanted else {
            return Ok(());
        };
        if self.current_frame == Some(wanted) {
            return Ok(());
        }
        while self.ahead.occupancy() > 0 {
            let Some(picture) = self.ahead.next_frame()? else {
                self.eos = true;
                break;
            };
            let landed = self.index.frame_at(picture.pts());
            let reached = landed.is_some_and(|frame| frame >= wanted);
            if !reached {
                // A picture the playhead has already run past. It was decoded
                // and will never be shown, which is what the preview counts as
                // a dropped frame.
                self.skipped = self.skipped.saturating_add(1);
            }
            self.current = Some(picture);
            self.current_frame = landed;
            if reached {
                break;
            }
        }
        Ok(())
    }

    /// Whether the picture on screen is the one that was asked for.
    const fn on_target(&self) -> bool {
        match (self.wanted, self.current_frame) {
            (Some(wanted), Some(current)) => wanted == current,
            (None, _) => true,
            _ => false,
        }
    }

    /// Whether polling this clip again can still bring a better picture.
    ///
    /// This, not [`Self::on_target`], is what the window waits on. Off target
    /// is not the same thing as *pending*: a worker that has hit the end of
    /// the file, and one that has already stepped past the frame that was
    /// asked for, will never deliver that frame however many times they are
    /// polled. Waiting on those is an unbounded repaint loop — a core burnt in
    /// the real window, and a harness that never stops painting in a test —
    /// so what is on screen is taken as the best this clip will show until
    /// the playhead moves and a fresh seek is posted.
    const fn pending(&self) -> bool {
        if self.on_target() {
            return false;
        }
        if self.eos {
            return false;
        }
        match (self.wanted, self.current_frame) {
            // The decoder walks forwards; a picture past the wanted one only
            // comes back by seeking, which the next `request` does.
            (Some(wanted), Some(current)) => current < wanted,
            // Nothing delivered yet: the ring is still filling.
            _ => true,
        }
    }

    /// Uploads the current picture if it is not already on the GPU, and hands
    /// back the view to sample.
    ///
    /// Returns `None` when there is no picture yet, and the error when the
    /// frame is not a usable NV12 picture.
    fn upload(&mut self, context: &RenderContext) -> SubResult<Option<SourceFrame>> {
        let Some(picture) = &self.current else {
            return Ok(None);
        };
        if self.uploaded == self.current_frame
            && let Some(view) = &self.view
            && let Some(converter) = &self.converter
        {
            let geometry = converter.geometry();
            return Ok(Some(SourceFrame::new(
                view.clone(),
                geometry.width(),
                geometry.height(),
            )));
        }
        let geometry = geometry_of(picture)?;
        // One converter per geometry, kept between frames: rebuilding it per
        // picture would build a render pipeline every frame.
        if self
            .converter
            .as_ref()
            .is_none_or(|converter| converter.geometry() != geometry)
        {
            self.converter = Some(Nv12Converter::new(context.device(), geometry));
            self.view = None;
        }
        let converter = self
            .converter
            .as_ref()
            .unwrap_or_else(|| unreachable!("just built"));
        converter
            .submit_frame(
                context.device(),
                context.queue(),
                picture.plane_data(0).unwrap_or(&[]),
                picture.plane_data(1).unwrap_or(&[]),
            )
            .map_err(|error| lift(&error))?;
        let view = self
            .view
            .get_or_insert_with(|| {
                converter
                    .output()
                    .create_view(&wgpu::TextureViewDescriptor::default())
            })
            .clone();
        self.uploaded = self.current_frame;
        Ok(Some(SourceFrame::new(
            view,
            geometry.width(),
            geometry.height(),
        )))
    }
}

/// What the service holds for one clip.
enum ClipSlot {
    /// A job is building the index and opening the pipeline.
    Opening {
        handle: JobHandle,
        slot: Arc<Mutex<Option<OpenedClip>>>,
        last_seen: u64,
    },
    /// The decoder is open and being driven.
    Ready(Box<ClipPreview>),
    /// The file could not be opened. Reported once and then left alone, so a
    /// broken file does not retry a pipeline build every frame.
    Failed { error: SubError, last_seen: u64 },
}

impl ClipSlot {
    /// The pass this slot was last wanted in.
    const fn last_seen(&self) -> u64 {
        match self {
            Self::Opening { last_seen, .. } | Self::Failed { last_seen, .. } => *last_seen,
            Self::Ready(preview) => preview.last_seen,
        }
    }

    /// Marks the slot as wanted in pass `seen`.
    const fn touch(&mut self, seen: u64) {
        match self {
            Self::Opening { last_seen, .. } | Self::Failed { last_seen, .. } => *last_seen = seen,
            Self::Ready(preview) => preview.last_seen = seen,
        }
    }
}

/// The decoded pictures behind the viewer.
///
/// One of these lives in the editor window. See the [module
/// documentation](self) for what it does and why.
pub struct PreviewService {
    clips: HashMap<ClipId, ClipSlot>,
    /// Which file a clip is previewed from: the proxy switch, as the viewer
    /// last left it.
    media_use: MediaUse,
    /// Where a built PTS index is cached, when the project has a folder.
    cache_dir: Option<PathBuf>,
    /// Whether the transport is running, which decides whether a forward step
    /// is served by the ring or by a seek.
    playing: bool,
    /// Which pass over the layers this is, for eviction.
    pass: u64,
    /// Whether the last pass had every layer's wanted picture in hand.
    settled: bool,
    /// How many layers the last pass handed the compositor.
    showing: usize,
    delivered: u64,
    late: u64,
}

impl std::fmt::Debug for PreviewService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let stats = self.stats();
        f.debug_struct("PreviewService")
            .field("open", &stats.open)
            .field("opening", &stats.opening)
            .field("failed", &stats.failed)
            .field("playing", &self.playing)
            .finish_non_exhaustive()
    }
}

impl Default for PreviewService {
    fn default() -> Self {
        Self::new()
    }
}

impl PreviewService {
    /// A service with nothing open.
    #[must_use]
    pub fn new() -> Self {
        Self {
            clips: HashMap::new(),
            media_use: MediaUse::PREVIEW_PROXIES,
            cache_dir: None,
            playing: false,
            pass: 0,
            settled: true,
            showing: 0,
            delivered: 0,
            late: 0,
        }
    }

    /// Closes every decoder.
    ///
    /// Called when the project, the sequence or the file behind a clip
    /// changes: an open pipeline is bound to one file at one resolution, and
    /// nothing about it survives that.
    pub fn clear(&mut self) {
        self.clips.clear();
    }

    /// Which file preview reads. Changing it closes every decoder.
    pub fn set_media_use(&mut self, media_use: MediaUse) {
        if self.media_use != media_use {
            self.media_use = media_use;
            self.clear();
        }
    }

    /// Where a built PTS index is cached. Changing it closes every decoder.
    pub fn set_cache_dir(&mut self, cache_dir: Option<PathBuf>) {
        if self.cache_dir != cache_dir {
            self.cache_dir = cache_dir;
            self.clear();
        }
    }

    /// Whether the transport is running.
    ///
    /// Playback is fed by the decode-ahead ring and scrubbing by the seek
    /// path; see the [module documentation](self).
    pub const fn set_playing(&mut self, playing: bool) {
        self.playing = playing;
    }

    /// Whether the last pass left nothing worth waiting for.
    ///
    /// False while a decoder is opening for a clip under the playhead and
    /// while a seek or a ring is still on its way to the frame that was asked
    /// for. True as soon as every layer is showing the best picture it will
    /// ever show at this playhead: the frame it was asked for, or — for a
    /// clip whose file has run out, whose media is offline and for one whose
    /// decoder would not open — nothing more to come. A caller with no
    /// pointer — a test, or a host asked to photograph the window — waits on
    /// this rather than on a sleep, and anything that waited on the strict
    /// reading instead would wait forever on a file that cannot answer.
    #[must_use]
    pub const fn settled(&self) -> bool {
        self.settled
    }

    /// Why a clip has no picture, for every clip whose decoder would not open.
    ///
    /// A file that cannot be decoded is reported once and then left alone —
    /// retrying a pipeline build every frame would be its own failure — so
    /// this is where the reason stays readable.
    #[must_use]
    pub fn failures(&self) -> Vec<(ClipId, &SubError)> {
        let mut failures: Vec<(ClipId, &SubError)> = self
            .clips
            .iter()
            .filter_map(|(clip, slot)| match slot {
                ClipSlot::Failed { error, .. } => Some((*clip, error)),
                _ => None,
            })
            .collect();
        failures.sort_unstable_by_key(|(clip, _)| *clip);
        failures
    }

    /// What the preview is doing.
    #[must_use]
    pub fn stats(&self) -> PreviewStats {
        let mut stats = PreviewStats {
            delivered: self.delivered,
            late: self.late,
            showing: self.showing,
            ..PreviewStats::default()
        };
        for slot in self.clips.values() {
            match slot {
                ClipSlot::Opening { .. } => stats.opening += 1,
                ClipSlot::Failed { .. } => stats.failed += 1,
                ClipSlot::Ready(preview) => {
                    stats.open += 1;
                    stats.dropped = stats
                        .dropped
                        .saturating_add(preview.skipped)
                        .saturating_add(preview.ahead.stats().frames_dropped);
                }
            }
        }
        stats
    }

    /// Drives every clip under `time` and hands back the pictures that are
    /// ready.
    ///
    /// Nothing here waits: a clip still opening, still seeking or offline
    /// contributes no picture, and the caller composites what it got. The
    /// returned [`PreviewFrames`] says whether anything changed, so a window
    /// that has not moved its playhead still redraws when a picture lands.
    pub fn pictures(
        &mut self,
        context: &RenderContext,
        jobs: &JobService,
        project: &Project,
        project_dir: Option<&Path>,
        sequence: &Sequence,
        time: RationalTime,
    ) -> PreviewFrames {
        self.pass = self.pass.wrapping_add(1);
        let pass = self.pass;
        let mut out = PreviewFrames::default();
        self.settled = true;
        let Some(project_dir) = project_dir else {
            // A project with no folder of its own cannot resolve a relative
            // media path, so there is nothing to decode.
            self.clips.clear();
            return out;
        };

        let layers: Vec<ResolvedClip<'_>> = resolve_layers_at(sequence, time).collect();
        // The layers that name a file this machine can read. A clip whose
        // media is offline or missing is not a layer that is *waiting* for a
        // picture: it will never have one, and nothing downstream should hold
        // a frame back for it.
        let mut decodable = Vec::with_capacity(layers.len());
        for layer in &layers {
            let clip = layer.clip_id();
            let Some(path) = media_path(project, project_dir, self.media_use, layer) else {
                continue;
            };
            self.ensure_slot(jobs, clip, &path, pass);
            let Some(slot) = self.clips.get_mut(&clip) else {
                // Over the pipeline budget: this layer waits for the next
                // pass.
                self.settled = false;
                continue;
            };
            slot.touch(pass);
            Self::advance(slot, layer.source_time, self.playing, &path);
            decodable.push(clip);
        }

        for clip in decodable {
            let Some(slot) = self.clips.get_mut(&clip) else {
                continue;
            };
            let ClipSlot::Ready(preview) = slot else {
                // A decoder still opening will have a picture; one that failed
                // never will, and a window must not wait forever for it.
                self.settled &= matches!(slot, ClipSlot::Failed { .. });
                continue;
            };
            if !preview.on_target() {
                self.late = self.late.saturating_add(1);
            }
            if preview.pending() {
                self.settled = false;
                out.busy = true;
            }
            let changed = preview.uploaded != preview.current_frame;
            match preview.upload(context) {
                Ok(Some(frame)) => {
                    out.changed |= changed;
                    if changed {
                        self.delivered = self.delivered.saturating_add(1);
                    }
                    out.frames.insert(clip, frame);
                }
                // No picture to upload yet. Whether that is worth another
                // frame is `pending`'s answer above, not this one's: a clip
                // whose file has run out has no picture and never will.
                Ok(None) => {}
                Err(error) => {
                    log::warn!(
                        "the preview could not upload a picture: [{}] {}",
                        error.code,
                        error.message
                    );
                    self.clips.insert(
                        clip,
                        ClipSlot::Failed {
                            error,
                            last_seen: pass,
                        },
                    );
                }
            }
        }

        // Only a decoder this pass asked for is worth repainting on: a slot
        // still opening for a clip the playhead has left contributes no layer
        // whenever it lands, so waiting on it would be a repaint loop with
        // nothing on the other end of it.
        out.busy |= self
            .clips
            .values()
            .any(|slot| matches!(slot, ClipSlot::Opening { last_seen, .. } if *last_seen == pass));
        self.showing = out.frames.len();
        self.evict(pass);
        out
    }

    /// Opens a decoder for `clip` if it has none, within the pipeline budget.
    fn ensure_slot(&mut self, jobs: &JobService, clip: ClipId, path: &Path, pass: u64) {
        if self.clips.contains_key(&clip) {
            return;
        }
        if self.clips.len() >= MAX_OPEN_CLIPS {
            // Make room by dropping whatever this pass has not asked for; if
            // everything open is in use the newcomer waits for the next pass.
            self.evict_to(MAX_OPEN_CLIPS - 1, pass);
            if self.clips.len() >= MAX_OPEN_CLIPS {
                return;
            }
        }
        let slot: Arc<Mutex<Option<OpenedClip>>> = Arc::new(Mutex::new(None));
        let into = Arc::clone(&slot);
        let file = path.to_path_buf();
        let cache_dir = self.cache_dir.clone();
        let handle = jobs.submit(
            PREVIEW_JOB_KIND,
            Priority::Interactive,
            move |ctx: &JobContext| {
                ctx.check()?;
                let opened = open_clip(&file, cache_dir, ctx)?;
                *into.lock().unwrap_or_else(PoisonError::into_inner) = Some(opened);
                Ok(())
            },
        );
        self.clips.insert(
            clip,
            ClipSlot::Opening {
                handle,
                slot,
                last_seen: pass,
            },
        );
    }

    /// Moves a finished open into place and drives whatever is ready.
    fn advance(slot: &mut ClipSlot, source_time: RationalTime, playing: bool, path: &Path) {
        if let ClipSlot::Opening {
            handle,
            slot: opened,
            last_seen,
        } = slot
        {
            if !handle.is_finished() {
                return;
            }
            let outcome = handle.wait();
            let taken = opened.lock().unwrap_or_else(PoisonError::into_inner).take();
            *slot = match (outcome.into_result(), taken) {
                (Ok(()), Some(opened)) => {
                    log::info!("preview decoder open for {}", path.display());
                    ClipSlot::Ready(Box::new(ClipPreview::new(opened, *last_seen)))
                }
                (Err(error), _) => {
                    log::warn!(
                        "no preview for {}: [{}] {}",
                        path.display(),
                        error.code,
                        error.message
                    );
                    ClipSlot::Failed {
                        error,
                        last_seen: *last_seen,
                    }
                }
                (Ok(()), None) => ClipSlot::Failed {
                    error: SubError::new(
                        sub_media::codes::DECODE_FAILED,
                        "the preview decoder reported success without a pipeline",
                    ),
                    last_seen: *last_seen,
                },
            };
        }
        let ClipSlot::Ready(preview) = slot else {
            return;
        };
        let driven = preview
            .request(source_time, playing)
            .and_then(|()| preview.poll());
        if let Err(error) = driven {
            log::warn!(
                "the preview decoder for {} stopped: [{}] {}",
                path.display(),
                error.code,
                error.message
            );
            let last_seen = preview.last_seen;
            *slot = ClipSlot::Failed { error, last_seen };
        }
    }

    /// Drops what this pass did not ask for, down to the pipeline budget.
    fn evict(&mut self, pass: u64) {
        self.evict_to(MAX_OPEN_CLIPS, pass);
    }

    /// Drops least-recently-wanted slots until at most `keep` remain, never
    /// touching one this pass asked for.
    fn evict_to(&mut self, keep: usize, pass: u64) {
        while self.clips.len() > keep {
            let Some(oldest) = self
                .clips
                .iter()
                .filter(|(_, slot)| slot.last_seen() != pass)
                .min_by_key(|(_, slot)| slot.last_seen())
                .map(|(clip, _)| *clip)
            else {
                return;
            };
            self.clips.remove(&oldest);
        }
    }
}

/// Builds the index and opens the pipeline for one clip. Worker thread only.
fn open_clip(path: &Path, cache_dir: Option<PathBuf>, ctx: &JobContext) -> SubResult<OpenedClip> {
    // The index is what makes the seek path frame-accurate and cheap: with one
    // set, a scrub step decodes on rather than flushing when that is cheaper,
    // aims an unavoidable seek at the exact keyframe, and keeps the pictures it
    // decoded in the decoder's GOP cache (TASK-133). It is also minutes of
    // parse on a long file, which is why this whole function is a job.
    let index =
        LazyPtsIndex::new(path.to_path_buf(), cache_dir).get_cancellable(&ctx.cancel_token())?;
    ctx.check()?;
    let options = DecoderOptions {
        streams: StreamSelection::Video,
        format: FrameFormat::Nv12,
        ..DecoderOptions::default()
    };
    let mut decoder = Decoder::open_with(path, options)?;
    decoder.set_index(Arc::clone(&index));
    Ok(OpenedClip {
        ahead: DecodeAhead::with_decoder(decoder, DECODE_AHEAD_FRAMES),
        index,
    })
}

/// The file a layer is previewed from, or `None` when the project does not
/// hold its media or the file is not there.
fn media_path(
    project: &Project,
    project_dir: &Path,
    media_use: MediaUse,
    layer: &ResolvedClip<'_>,
) -> Option<PathBuf> {
    let item: &MediaItem = project
        .media
        .iter()
        .find(|item| item.id == layer.clip.media)?;
    if item.offline {
        return None;
    }
    let path = item.absolute_source(project_dir, media_use);
    path.is_file().then_some(path)
}

/// The NV12 geometry of a decoded picture.
fn geometry_of(frame: &VideoFrame) -> SubResult<Nv12Geometry> {
    if frame.format() != FrameFormat::Nv12 {
        return Err(SubError::new(
            sub_media::codes::DECODE_FAILED,
            format!(
                "the preview needs NV12 pictures; the decoder produced {}",
                frame.format().as_str()
            ),
        ));
    }
    Nv12Geometry::new(
        frame.width(),
        frame.height(),
        frame.plane_stride(0).unwrap_or(0),
        frame.plane_stride(1).unwrap_or(0),
    )
    .map_err(|error| lift(&error))
}

/// Carries a [`RenderError`] across as a [`SubError`], keeping its code.
fn lift(error: &RenderError) -> SubError {
    sub_export::sequence::lift_render_error(error)
}

#[cfg(test)]
mod tests {
    use super::{MAX_OPEN_CLIPS, PreviewFrames, PreviewService, PreviewStats, Step, plan_step};
    use sub_model::MediaUse;

    /// The ring depth every planning case below is judged against.
    const RING: usize = super::DECODE_AHEAD_FRAMES;

    #[test]
    fn a_scrub_step_always_seeks_so_the_index_and_the_gop_cache_apply() {
        // Forward by one, forward by three, backwards, and a jump: while the
        // transport is parked every one of them goes through the seek path,
        // which is Decoder::seek_to with a PtsIndex set.
        for wanted in [11_usize, 13, 5, 900] {
            assert_eq!(
                plan_step(Some(10), wanted, false, false, RING),
                Step::Seek,
                "a scrub to frame {wanted} must seek"
            );
        }
    }

    #[test]
    fn playback_takes_a_forward_step_out_of_the_ring() {
        assert_eq!(plan_step(Some(10), 11, true, false, RING), Step::Ring);
        assert_eq!(
            plan_step(Some(10), 10 + RING, true, false, RING),
            Step::Ring,
            "the far end of the ring is still in reach"
        );
    }

    #[test]
    fn playback_seeks_for_anything_the_ring_cannot_reach() {
        assert_eq!(
            plan_step(Some(10), 11 + RING, true, false, RING),
            Step::Seek,
            "one past the ring is a jump"
        );
        assert_eq!(
            plan_step(Some(10), 9, true, false, RING),
            Step::Seek,
            "reverse play cannot come out of a forward ring"
        );
        assert_eq!(
            plan_step(Some(10), 11, true, true, RING),
            Step::Seek,
            "a worker parked at end of stream has to be restarted by a seek"
        );
        assert_eq!(
            plan_step(None, 0, true, false, RING),
            Step::Seek,
            "nothing has been delivered, so nothing can be popped"
        );
    }

    #[test]
    fn the_picture_already_on_screen_is_left_alone() {
        assert_eq!(plan_step(Some(10), 10, true, false, RING), Step::Hold);
        assert_eq!(plan_step(Some(10), 10, false, false, RING), Step::Hold);
    }

    const fn _assert_send() {
        const fn is_send<T: Send>() {}
        // The decoders are opened on a worker and moved to the UI thread, so
        // everything the job hands over must cross a thread boundary.
        is_send::<super::OpenedClip>();
    }

    #[test]
    fn a_fresh_service_holds_nothing() {
        let service = PreviewService::new();
        let stats = service.stats();
        assert_eq!(stats, PreviewStats::default());
        assert!(format!("{service:?}").contains("playing: false"));
    }

    #[test]
    fn changing_the_proxy_switch_closes_the_decoders() {
        let mut service = PreviewService::new();
        assert_eq!(service.media_use, MediaUse::PREVIEW_PROXIES);
        service.set_media_use(MediaUse::PREVIEW_ORIGINALS);
        assert_eq!(service.media_use, MediaUse::PREVIEW_ORIGINALS);
        // Setting the same value again is not a reason to reopen anything.
        service.set_media_use(MediaUse::PREVIEW_ORIGINALS);
        assert_eq!(service.media_use, MediaUse::PREVIEW_ORIGINALS);
    }

    #[test]
    fn the_pipeline_budget_leaves_room_for_a_crossfade() {
        // A dissolve resolves two layers at once, so a budget below two would
        // make a transition impossible to preview whole.
        const _: () = assert!(MAX_OPEN_CLIPS >= 2);
        // And the ring has to be able to hold a step, or a forward step could
        // never be served out of it.
        const _: () = assert!(super::DECODE_AHEAD_FRAMES >= 1);
    }

    #[test]
    fn an_empty_frame_set_composites_to_black() {
        let frames = PreviewFrames::default();
        assert!(frames.is_empty());
        assert_eq!(frames.len(), 0);
        assert!(!frames.changed());
        assert!(!frames.busy());
    }
}
