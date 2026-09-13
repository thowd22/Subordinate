//! Bounded, worker-decoded audio windows for the live timeline mixer.
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};

use sub_audio::mixer::{
    ClipSpec, MixGraph, MixGraphBuilder, Mixer, MixerConfig, MixerControl, TrackSpec, mixer,
};
use sub_core::{SubError, SubResult, codes};
use sub_model::{ContentHash, MediaItem, MediaUse, Project, Sequence, TrackKind};
use sub_time::{Rational, RationalTime, Rounding};

const WINDOW_SECONDS: u64 = 4;
const STEP_SECONDS: u64 = 2;
const MAX_CACHE_SAMPLES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key {
    path: PathBuf,
    hash: Option<ContentHash>,
    stream: u16,
    sample_rate: u32,
    start: u64,
}

struct DecodeJob {
    key: Key,
    item: MediaItem,
    directory: PathBuf,
    epoch: u64,
}

struct Decoded {
    key: Key,
    samples: SubResult<Arc<[f32]>>,
    epoch: u64,
}

struct Slot {
    track: usize,
    key: Key,
    item: MediaItem,
    source_start: usize,
}

/// UI-owned live audio graph. Decoding is restricted to four-second windows
/// around active/upcoming clips, with a 64 MiB PCM cache and seek cancellation.
/// Mixer callbacks address immutable PCM by source frame, so late loads never
/// replay an old FIFO position and scrubbing can revisit an already loaded window.
pub struct PlaybackAudio {
    graph: Arc<MixGraph>,
    configuration_error: Option<String>,
    slots: Vec<Slot>,
    directory: PathBuf,
    cache: HashMap<Key, Arc<[f32]>>,
    pending: HashSet<Key>,
    wanted: HashSet<Key>,
    selected: Vec<Option<Key>>,
    failed: HashMap<Key, String>,
    jobs: mpsc::SyncSender<DecodeJob>,
    results: mpsc::Receiver<Decoded>,
    epoch: Arc<AtomicU64>,
    position: u64,
}

impl PlaybackAudio {
    /// Starts a bounded decode worker with an empty graph.
    ///
    /// # Errors
    /// Returns a graph or thread creation error.
    pub fn new(sample_rate: u32) -> SubResult<Self> {
        Self::with_decoder(sample_rate, decode_window)
    }

    fn with_decoder(
        sample_rate: u32,
        decode: impl Fn(&DecodeJob, &dyn Fn() -> bool) -> SubResult<Arc<[f32]>> + Send + 'static,
    ) -> SubResult<Self> {
        let (jobs, requests) = mpsc::sync_channel::<DecodeJob>(64);
        let (completed, results) = mpsc::channel();
        let epoch = Arc::new(AtomicU64::new(0));
        let worker_epoch = Arc::clone(&epoch);
        std::thread::Builder::new()
            .name("playback-audio".to_owned())
            .spawn(move || {
                while let Ok(job) = requests.recv() {
                    let cancelled = || worker_epoch.load(Ordering::Relaxed) != job.epoch;
                    if cancelled() {
                        continue;
                    }
                    let samples = decode(&job, &cancelled);
                    if !cancelled()
                        && completed
                            .send(Decoded {
                                key: job.key,
                                samples,
                                epoch: job.epoch,
                            })
                            .is_err()
                    {
                        break;
                    }
                }
            })
            .map_err(|error| {
                SubError::wrap(
                    codes::INTERNAL,
                    "could not start playback audio worker",
                    &error,
                )
            })?;
        Ok(Self {
            graph: MixGraphBuilder::new(sample_rate, 2).build()?,
            configuration_error: None,
            slots: Vec::new(),
            directory: PathBuf::new(),
            cache: HashMap::new(),
            pending: HashSet::new(),
            wanted: HashSet::new(),
            selected: Vec::new(),
            failed: HashMap::new(),
            jobs,
            results,
            epoch,
            position: 0,
        })
    }

    /// Rebuilds gains, routing and placements without decoding on this thread.
    ///
    /// # Errors
    /// Returns an invalid graph or missing media error.
    pub fn configure(
        &mut self,
        project: &Project,
        sequence: &Sequence,
        directory: &Path,
    ) -> SubResult<()> {
        self.configuration_error = None;
        if let Err(error) = self.configure_graph(project, sequence, directory) {
            self.clear_with_error(format!("[{}] {}", error.code, error.message))?;
            return Err(error);
        }
        Ok(())
    }

    /// Clears previous project audio when the new project cannot be resolved.
    ///
    /// # Errors
    /// Returns an error if the current stream format cannot form an empty graph.
    pub fn clear_with_error(&mut self, message: impl Into<String>) -> SubResult<()> {
        self.cancel_pending();
        self.slots.clear();
        self.selected.clear();
        self.wanted.clear();
        self.cache.clear();
        self.failed.clear();
        self.configuration_error = Some(message.into());
        self.graph = MixGraphBuilder::new(self.graph.sample_rate(), 2).build()?;
        Ok(())
    }

    fn configure_graph(
        &mut self,
        project: &Project,
        sequence: &Sequence,
        directory: &Path,
    ) -> SubResult<()> {
        let sample_rate = sequence.settings.sample_rate;
        let audio_rate = Rational::new(sample_rate, 1)
            .ok_or_else(|| SubError::new(codes::INVALID_ARGUMENT, "invalid audio sample rate"))?;
        let mut graph = MixGraphBuilder::new(sample_rate, 2);
        let mut slots = Vec::new();
        for (track_index, track) in sequence
            .tracks
            .iter()
            .filter(|track| track.kind == TrackKind::Audio)
            .enumerate()
        {
            let mut lane = TrackSpec::new()
                .with_gain_db(track.gain.decibels().as_f64())
                .with_muted(track.muted)
                .with_solo(track.solo);
            for (clip, placement) in track.clip_placements(sequence.settings.frame_rate) {
                let item = project.media_item(clip.media).ok_or_else(|| {
                    SubError::new(codes::NOT_FOUND, "audio clip references missing media")
                })?;
                let key = Key {
                    path: item.absolute_source(directory, MediaUse::Export),
                    hash: item.hash,
                    stream: clip.audio_stream,
                    sample_rate,
                    start: 0,
                };
                let source_start = clip
                    .source_range
                    .start()
                    .checked_rescaled_to_rounding(audio_rate, Rounding::Floor)
                    .and_then(|time| usize::try_from(time.value()).ok())
                    .ok_or_else(|| {
                        SubError::new(
                            codes::INVALID_ARGUMENT,
                            "audio clip start is not representable",
                        )
                    })?;
                lane = lane.with_clip(
                    ClipSpec::new(slots.len(), placement.start(), placement.duration())
                        .with_gain_db(clip.gain.decibels().as_f64())
                        .with_fades(clip.fade_in, clip.fade_out),
                );
                slots.push(Slot {
                    track: track_index,
                    key,
                    item: item.clone(),
                    source_start,
                });
            }
            graph = graph.track(lane);
        }
        self.graph = graph.slots(slots.len()).build()?;
        self.slots = slots;
        self.directory = directory.to_path_buf();
        self.selected = vec![None; self.slots.len()];
        self.cancel_pending();
        self.failed.clear();
        self.wanted.clear();
        Ok(())
    }

    fn cancel_pending(&mut self) {
        self.epoch.fetch_add(1, Ordering::Relaxed);
        self.pending.clear();
    }

    fn accept_completed(&mut self) {
        while let Ok(decoded) = self.results.try_recv() {
            if decoded.epoch != self.epoch.load(Ordering::Relaxed) {
                continue;
            }
            self.pending.remove(&decoded.key);
            if !self.wanted.contains(&decoded.key) {
                continue;
            }
            match decoded.samples {
                Ok(samples) => {
                    self.cache.insert(decoded.key, samples);
                }
                Err(error) => {
                    self.failed
                        .insert(decoded.key, format!("[{}] {}", error.code, error.message));
                }
            }
        }
    }

    /// Requests only nearby windows, accepts completed work, and reports whether
    /// the active mixer's slots need replacing. Safe to call every UI frame.
    pub fn update(&mut self, position: RationalTime) -> bool {
        let frame = position
            .checked_rescaled_to_rounding(self.graph.rate(), Rounding::Floor)
            .and_then(|time| u64::try_from(time.value()).ok())
            .unwrap_or(0);
        let rate = u64::from(self.graph.sample_rate());
        if self.position.abs_diff(frame) > rate / 2 {
            self.cancel_pending();
        }
        self.position = frame;
        self.wanted.clear();
        let mut needed = Vec::new();
        for (index, slot) in self.slots.iter().enumerate() {
            if self.graph.track_audible(slot.track) != Some(true) {
                self.selected[index] = None;
                continue;
            }
            let Some((start, length)) = self.graph.slot_span(index) else {
                continue;
            };
            let end = start.saturating_add(length);
            if frame >= end || frame.saturating_add(rate * STEP_SECONDS) < start {
                continue;
            }
            let source = u64::try_from(slot.source_start)
                .unwrap_or(u64::MAX)
                .saturating_add(frame.saturating_sub(start));
            let mut key = slot.key.clone();
            let grid = source / (rate * STEP_SECONDS) * (rate * STEP_SECONDS);
            key.start = grid.saturating_sub(rate / 10);
            self.wanted.insert(key.clone());
            needed.push((key.clone(), slot.item.clone()));
            if source.saturating_add(rate) >= grid + rate * STEP_SECONDS {
                key.start = (grid + rate * STEP_SECONDS).saturating_sub(rate / 10);
                self.wanted.insert(key.clone());
                needed.push((key, slot.item.clone()));
            }
        }
        self.accept_completed();
        let before = self.selected.clone();
        self.select_windows(frame, rate);
        self.cache.retain(|key, _| {
            self.wanted.contains(key)
                || self
                    .selected
                    .iter()
                    .any(|selected| selected.as_ref() == Some(key))
        });
        self.failed.retain(|key, _| self.wanted.contains(key));
        for (key, item) in needed {
            if self.cache.contains_key(&key)
                || self.pending.contains(&key)
                || self.failed.contains_key(&key)
            {
                continue;
            }
            let held: usize = self.cache.values().map(|samples| samples.len()).sum();
            let reserved = self
                .pending
                .len()
                .saturating_mul(usize::try_from(rate * WINDOW_SECONDS * 2).unwrap_or(usize::MAX));
            if held
                .saturating_add(reserved)
                .saturating_add(usize::try_from(rate * WINDOW_SECONDS * 2).unwrap_or(usize::MAX))
                > MAX_CACHE_SAMPLES
            {
                self.failed
                    .insert(key, "preview audio cache limit reached".to_owned());
                continue;
            }
            let job = DecodeJob {
                key: key.clone(),
                item,
                directory: self.directory.clone(),
                epoch: self.epoch.load(Ordering::Relaxed),
            };
            if self.jobs.try_send(job).is_ok() {
                self.pending.insert(key);
            }
        }
        before != self.selected
    }

    fn select_windows(&mut self, frame: u64, rate: u64) {
        for (index, slot) in self.slots.iter().enumerate() {
            if self.graph.track_audible(slot.track) != Some(true) {
                self.selected[index] = None;
                continue;
            }
            let Some((start, length)) = self.graph.slot_span(index) else {
                continue;
            };
            let source = u64::try_from(slot.source_start)
                .unwrap_or(u64::MAX)
                .saturating_add(frame.saturating_sub(start));
            let end = start.saturating_add(length);
            self.selected[index] = if frame < end
                && frame.saturating_add(rate * STEP_SECONDS) >= start
            {
                self.cache
                    .keys()
                    .filter(|key| {
                        key.path == slot.key.path
                            && key.hash == slot.key.hash
                            && key.stream == slot.key.stream
                            && key.sample_rate == slot.key.sample_rate
                            && key
                                .start
                                .saturating_add(if key.start == 0 { 0 } else { rate / 10 })
                                <= source
                            && source < key.start + WINDOW_SECONDS * rate
                    })
                    .max_by_key(|key| key.start)
                    .cloned()
            } else {
                None
            };
        }
    }

    /// Whether every currently active audio clip has an available PCM window.
    #[must_use]
    pub fn ready(&self) -> bool {
        if self.configuration_error.is_some() {
            return false;
        }
        self.selected.iter().enumerate().all(|(index, selected)| {
            self.graph.slot_span(index).is_none_or(|(start, length)| {
                self.graph.track_audible(self.slots[index].track) != Some(true)
                    || self.position < start
                    || self.position >= start.saturating_add(length)
                    || selected.is_some()
            })
        })
    }

    /// Whether active/upcoming audio is still being prepared.
    #[must_use]
    pub fn loading(&self) -> bool {
        !self.pending.is_empty()
    }

    /// A current decode failure for display beside the playback controls.
    #[must_use]
    pub fn error(&self) -> Option<&str> {
        if let Some(error) = self.configuration_error.as_deref() {
            return Some(error);
        }
        self.wanted
            .iter()
            .find_map(|key| self.failed.get(key).map(String::as_str))
    }

    /// Installs selected immutable windows; allocation retirement stays off callback.
    ///
    /// # Errors
    /// Returns a mixer slot/update queue error.
    pub fn install_ready(&self, control: &mut MixerControl) -> SubResult<()> {
        control.collect_retired();
        for (index, slot) in self.slots.iter().enumerate() {
            if let Some(key) = self.selected[index].as_ref()
                && let Some(samples) = self.cache.get(key)
            {
                control.install_pcm(
                    index,
                    Arc::clone(samples),
                    slot.source_start,
                    usize::try_from(key.start).unwrap_or(usize::MAX),
                )?;
            } else {
                control.clear_slot(index)?;
            }
        }
        Ok(())
    }

    /// Builds the same callback mixer as the audio device uses.
    ///
    /// # Errors
    /// Returns a graph/update queue error.
    pub fn create_mixer(&self) -> SubResult<(MixerControl, Mixer)> {
        let config = MixerConfig {
            slot_capacity: self.slots.len().max(1),
            queue_capacity: self.slots.len().saturating_mul(2).saturating_add(8).max(32),
            ..MixerConfig::default()
        };
        let (mut control, mixer) = mixer(Arc::clone(&self.graph), config)?;
        self.install_ready(&mut control)?;
        Ok((control, mixer))
    }
}

impl Drop for PlaybackAudio {
    fn drop(&mut self) {
        self.cancel_pending();
    }
}

fn decode_window(job: &DecodeJob, cancelled: &dyn Fn() -> bool) -> SubResult<Arc<[f32]>> {
    let Some(rate) = Rational::new(job.key.sample_rate, 1) else {
        return Err(SubError::new(
            codes::INVALID_ARGUMENT,
            "invalid sample rate",
        ));
    };
    let start = RationalTime::new(i64::try_from(job.key.start).unwrap_or(i64::MAX), rate);
    let duration = RationalTime::new(
        i64::from(job.key.sample_rate) * i64::try_from(WINDOW_SECONDS).unwrap_or(4),
        rate,
    );
    sub_export::sequence::decode_media_pcm_window(
        &job.item,
        &job.directory,
        job.key.stream,
        job.key.sample_rate,
        2,
        start,
        duration,
        cancelled,
    )
    .map(|pcm| Arc::<[f32]>::from(pcm.samples))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    use sub_model::{Clip, MediaPath, SequenceSettings, Track, TrackItem};
    use sub_time::TimeRange;

    #[test]
    fn configuration_failure_clears_previous_audio_and_surfaces_the_error() {
        let mut audio = PlaybackAudio::new(48_000).unwrap();
        let project = Project::new("Missing media");
        let mut sequence = Sequence::new("Main", SequenceSettings::default());
        let item = MediaItem::new(MediaPath::new("missing.wav").unwrap());
        let mut track = Track::new("A1", TrackKind::Audio);
        track.items.push(TrackItem::Clip(Clip::new(
            "Missing",
            item.id,
            TimeRange::new(
                RationalTime::new(0, Rational::ONE),
                RationalTime::new(1, Rational::ONE),
            )
            .unwrap(),
        )));
        sequence.tracks.push(track);
        assert!(
            audio
                .configure(&project, &sequence, Path::new("/tmp"))
                .is_err()
        );
        audio.update(RationalTime::new(0, Rational::ONE));
        assert!(!audio.ready());
        assert!(audio.error().is_some());
        assert_eq!(audio.graph.duration_frames(), 0);
        assert!(audio.slots.is_empty());
    }

    #[test]
    fn ninety_minute_sources_seek_directly_to_bounded_tail_windows() {
        let requested = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&requested);
        let mut audio = PlaybackAudio::with_decoder(48_000, move |job, _| {
            observed.lock().unwrap().push(job.key.start);
            Ok(vec![0.25; 48_000 * 4 * 2].into())
        })
        .unwrap();
        let mut project = Project::new("Long playlist");
        let mut sequence = Sequence::new("Main", SequenceSettings::default());
        sequence.settings.frame_rate = Rational::FPS_60;
        for index in 0..64 {
            let item = MediaItem::new(MediaPath::new(format!("long-{index}.wav")).unwrap());
            let mut track = Track::new(format!("A{index}"), TrackKind::Audio);
            track.items.push(TrackItem::Clip(Clip::new(
                "Long source",
                item.id,
                TimeRange::new(
                    RationalTime::new(0, Rational::FPS_60),
                    RationalTime::new(90 * 60 * 60, Rational::FPS_60),
                )
                .unwrap(),
            )));
            sequence.tracks.push(track);
            project.media.push(item);
        }
        audio
            .configure(&project, &sequence, Path::new("/tmp"))
            .unwrap();
        assert!(
            requested.lock().unwrap().is_empty(),
            "configuring must not eagerly decode"
        );
        let tail = RationalTime::new(5_398, Rational::ONE);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            audio.update(tail);
            let held: usize = audio.cache.values().map(|pcm| pcm.len()).sum();
            assert!(held + audio.pending.len() * 48_000 * 4 * 2 <= MAX_CACHE_SAMPLES);
            if !audio.loading() {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        let requests = requested.lock().unwrap();
        assert!(!requests.is_empty());
        assert!(
            requests.len() < 64,
            "aggregate PCM budget must limit concurrent clips"
        );
        assert!(
            requests.iter().all(|start| *start >= 5_397 * 48_000),
            "tail seek must never decode from the source origin"
        );
        assert!(audio.error().is_some(), "exhausted cache budget is visible");
    }
}
