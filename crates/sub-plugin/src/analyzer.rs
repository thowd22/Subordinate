//! Running analyzer plugins as background jobs (docs/PLAN.md §6.2, §5.2).
//!
//! An analyzer is the `analyzer` world of `wit/subordinate-plugin.wit`: it
//! exports `analyze(media, options)` and imports `analysis-host`, the progress
//! and cancellation channel. Nothing about that world is allowed to run on the
//! engine thread — scene detection or a transcript takes minutes — so the host
//! runs every analysis as one job on the shared
//! [`JobService`](sub_core::jobs::JobService), which gives the run three
//! things for free: a place in the priority queue, progress events the UI and
//! the MCP bridge already draw, and cooperative cancellation.
//!
//! What the job does is fixed here, not left to each analyzer:
//!
//! 1. call the plugin, handing it an [`AnalysisContext`] that forwards
//!    `report-progress` and answers `is-cancelled`;
//! 2. check for cancellation once more — a run that was stopped stores
//!    nothing, however much it found;
//! 3. turn the WIT findings into a [`Analysis`](sub_model::Analysis), assigning
//!    the marker identifiers, so a plugin never invents one;
//! 4. store them on the media item by applying
//!    [`SetMediaAnalysis`](sub_edit::commands::SetMediaAnalysis) through the
//!    engine.
//!
//! Step 4 is the important one: findings reach the project the same way every
//! other mutation does, as one undoable command on the engine's queue
//! (decision-7). An analyzer holds no handle on the project model, and an
//! analysis that lands can be undone.
//!
//! [`Analyzer`] is the seam the WASM instance sits behind. TASK-84 owns the
//! wasmtime store, fuel and epoch limits that make one; everything here is
//! about the job around it, and is exercised with plain Rust analyzers.

use std::sync::Arc;

use sub_core::SubResult;
use sub_core::jobs::{JobContext, JobHandle, JobService, Priority};
use sub_edit::EngineHandle;
use sub_edit::commands::SetMediaAnalysis;
use sub_model::MediaId;

use crate::bindings::analyzer::subordinate::plugin::analysis::AnalysisResult as WitAnalysisResult;
use crate::convert::analysis;

/// The `kind` every analysis job carries on the queue, and the one events name.
pub const JOB_KIND: &str = "analysis";

/// The host side of the WIT `analysis-host` import, for one run.
///
/// Borrowed rather than owned: it lives exactly as long as the call into the
/// plugin, which is what makes "is this run cancelled" a question about the
/// job, not about the analyzer.
#[derive(Debug)]
pub struct AnalysisContext<'a> {
    job: &'a JobContext,
}

impl<'a> AnalysisContext<'a> {
    /// Wraps the context of the job running the analysis.
    #[must_use]
    pub fn new(job: &'a JobContext) -> Self {
        Self { job }
    }

    /// The job this analysis runs as.
    #[must_use]
    pub fn job(&self) -> &JobContext {
        self.job
    }

    /// Publishes progress: `done` units finished out of `total`, where a
    /// `total` of zero means the size of the work is not known yet.
    pub fn report_progress(&self, done: u64, total: u64) {
        self.job.progress(done, total);
    }

    /// Whether the host has asked this run to stop.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.job.is_cancelled()
    }

    /// `Ok(())` while the run is live, `core.cancelled` once it is not.
    ///
    /// # Errors
    ///
    /// Returns [`JobContext::cancelled`] when cancellation has been asked for.
    pub fn check(&self) -> SubResult<()> {
        self.job.check()
    }
}

/// One analyzer, ready to be run against a media item.
///
/// This is the `analyze` export of the `analyzer` world with the host's
/// machinery in place of the guest's: a WASM instance implements it by calling
/// into the component, and a test implements it directly. It is `Send` and
/// owned by the job, because the job is what runs it, on a worker thread.
pub trait Analyzer: Send + 'static {
    /// Analyses `media` and returns what was found.
    ///
    /// `options` is a JSON object in the analyzer's own schema, the encoding
    /// the Command API uses for parameters. Long runs must call
    /// [`AnalysisContext::report_progress`] as they go and stop promptly once
    /// [`AnalysisContext::is_cancelled`] is true.
    ///
    /// # Errors
    ///
    /// Whatever the analysis failed with. `core.cancelled` is the conventional
    /// code for a run that stopped because it was asked to.
    fn analyze(
        &mut self,
        media: MediaId,
        options: &str,
        context: &AnalysisContext<'_>,
    ) -> SubResult<WitAnalysisResult>;
}

/// The queue analyzer runs go through.
///
/// Holds the two things a run needs and nothing else: the job service that
/// schedules it, and the engine handle its findings are stored through. One of
/// these is shared by the whole host — analyses compete for workers with
/// thumbnails and proxies, and are meant to.
#[derive(Debug, Clone)]
pub struct AnalysisJobs {
    jobs: Arc<JobService>,
    engine: EngineHandle,
}

impl AnalysisJobs {
    /// Runs analyses on `jobs`, storing their findings through `engine`.
    #[must_use]
    pub fn new(jobs: Arc<JobService>, engine: EngineHandle) -> Self {
        Self { jobs, engine }
    }

    /// Queues `analyzer` against `media` and returns at once.
    ///
    /// The returned [`JobHandle`] is how the run is watched and stopped:
    /// [`JobHandle::cancel`] on a job that has not started keeps it from ever
    /// running, and on one that has asks it to stop. Either way nothing is
    /// stored.
    ///
    /// `analyzer_id` is the identity the findings are filed under on the media
    /// item, so re-running the same analyzer replaces its previous findings
    /// rather than accumulating them.
    pub fn submit<A: Analyzer>(
        &self,
        analyzer_id: impl Into<String>,
        media: MediaId,
        options: impl Into<String>,
        priority: Priority,
        analyzer: A,
    ) -> JobHandle {
        let analyzer_id = analyzer_id.into();
        let options = options.into();
        let engine = self.engine.clone();
        let mut analyzer = analyzer;

        self.jobs.submit(JOB_KIND, priority, move |job| {
            let context = AnalysisContext::new(job);
            let found = analyzer.analyze(media, &options, &context)?;
            // A cancelled run stores nothing: an analyzer that returned early
            // has partial findings, and half a transcript on a media item is
            // worse than none.
            context.check()?;

            let analysis = analysis(&analyzer_id, &found)?;
            engine
                .apply(SetMediaAnalysis::new(media, analysis))
                .map(|_| ())
        })
    }

    /// Queues `analyzer` at [`Priority::Background`], where analyses belong
    /// unless a person is waiting on one.
    pub fn submit_background<A: Analyzer>(
        &self,
        analyzer_id: impl Into<String>,
        media: MediaId,
        options: impl Into<String>,
        analyzer: A,
    ) -> JobHandle {
        self.submit(analyzer_id, media, options, Priority::Background, analyzer)
    }

    /// The job service analyses are queued on.
    #[must_use]
    pub fn jobs(&self) -> &Arc<JobService> {
        &self.jobs
    }

    /// The engine findings are stored through.
    #[must_use]
    pub fn engine(&self) -> &EngineHandle {
        &self.engine
    }
}

/// A convenience for analyzers written in Rust and run in-process.
///
/// Implements [`Analyzer`] for a closure, so a first-party analyzer or a test
/// does not need a type of its own.
impl<F> Analyzer for F
where
    F: FnMut(MediaId, &str, &AnalysisContext<'_>) -> SubResult<WitAnalysisResult> + Send + 'static,
{
    fn analyze(
        &mut self,
        media: MediaId,
        options: &str,
        context: &AnalysisContext<'_>,
    ) -> SubResult<WitAnalysisResult> {
        self(media, options, context)
    }
}
