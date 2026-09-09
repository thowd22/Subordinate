//! The relink dialog: point offline items at the files they moved to.
//!
//! Projects move, drives are re-mounted and cards are re-copied, so the bin
//! ends up holding items whose files are no longer where the project says. The
//! dialog offers the two ways out (docs/PLAN.md §5.6): pick the replacement
//! file directly, or pick a folder and let the search find it — by content
//! hash first, and only then by name, which is what
//! [`sub_edit::relink`] decides.
//!
//! The search is where the work is: hashing a card of footage takes seconds,
//! so it runs on the [`JobService`] like every other file read in this crate,
//! and the dialog is pumped once a frame. The UI thread never hashes and never
//! walks a folder.
//!
//! Like the other panels here the dialog mutates nothing. It hands back a
//! [`RelinkPlan`], which the host applies through the history as a single
//! group, so relinking twenty items is one press of undo.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText, Ui};
use sub_core::{JobContext, JobHandle, JobService, Priority};
use sub_edit::relink::{
    MatchKind, RelinkMatch, RelinkPlan, RelinkTarget, SearchOptions, match_chosen, match_targets,
    scan_folder,
};
use sub_model::{MediaId, Project};

/// The job kind a folder search reports itself as.
pub const RELINK_JOB_KIND: &str = "relink";

/// A folder search running on the job service.
#[derive(Debug)]
struct Search {
    handle: JobHandle,
    folder: PathBuf,
    found: Arc<Mutex<Vec<RelinkMatch>>>,
}

/// The relink dialog.
///
/// Opened for one item from the bin's `Relink` button, or for every offline
/// item at once from `Relink all`. It owns nothing but the question being
/// asked: which items, which folder was searched, and what that search found.
#[derive(Debug, Default)]
pub struct RelinkDialog {
    open: bool,
    targets: Vec<RelinkTarget>,
    search: Option<Search>,
    matches: Vec<RelinkMatch>,
    searched: Option<PathBuf>,
    options: SearchOptions,
}

impl RelinkDialog {
    /// A closed dialog.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The same dialog, searching under `options`.
    #[must_use]
    pub fn with_options(mut self, options: SearchOptions) -> Self {
        self.options = options;
        self
    }

    /// Opens the dialog for one item, offline or not.
    pub fn open_for(&mut self, project: &Project, media: MediaId) {
        self.reset(RelinkTarget::one(project, media).into_iter().collect());
    }

    /// Opens the dialog for every offline item in `project`.
    pub fn open_for_offline(&mut self, project: &Project) {
        self.reset(RelinkTarget::offline(project));
    }

    /// Closes the dialog and cancels a search still running.
    pub fn close(&mut self) {
        if let Some(search) = &self.search {
            search.handle.cancel();
        }
        self.open = false;
        self.search = None;
    }

    /// Whether the dialog is on screen.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The items the dialog is relinking.
    #[must_use]
    pub fn targets(&self) -> &[RelinkTarget] {
        &self.targets
    }

    /// What the last search proposed.
    #[must_use]
    pub fn matches(&self) -> &[RelinkMatch] {
        &self.matches
    }

    /// True while a folder search is running.
    #[must_use]
    pub fn is_searching(&self) -> bool {
        self.search.is_some()
    }

    /// Starts a recursive search of `folder` on `jobs`.
    ///
    /// Returns at once; the matches land on a later [`RelinkDialog::poll`].
    /// A search already running is cancelled and replaced.
    pub fn search_folder(&mut self, jobs: &JobService, folder: impl Into<PathBuf>) {
        let folder = folder.into();
        if let Some(previous) = &self.search {
            previous.handle.cancel();
        }
        self.matches.clear();
        self.searched = None;

        let found = Arc::new(Mutex::new(Vec::new()));
        let slot = Arc::clone(&found);
        let targets = self.targets.clone();
        let options = self.options;
        let root = folder.clone();
        let handle = jobs.submit(
            RELINK_JOB_KIND,
            Priority::Interactive,
            move |ctx: &JobContext| {
                let files = scan_folder(&root, options);
                ctx.progress(1, 2);
                ctx.check()?;
                // The expensive half: every candidate is hashed here, on the
                // worker, never on the UI thread.
                let matches = match_targets(&targets, &files);
                *slot
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = matches;
                ctx.progress(2, 2);
                Ok(())
            },
        );
        self.search = Some(Search {
            handle,
            folder,
            found,
        });
    }

    /// Takes the file the user picked as the answer for the single item the
    /// dialog was opened for.
    ///
    /// An explicit choice is never second-guessed: the file is hashed so the
    /// item records what it now points at, and it is proposed whatever it is
    /// called.
    pub fn choose_file(&mut self, file: &Path) {
        self.searched = None;
        self.matches = self
            .targets
            .first()
            .map(|target| vec![match_chosen(target, file)])
            .unwrap_or_default();
    }

    /// Collects a finished search. Returns true when one landed this call.
    pub fn poll(&mut self) -> bool {
        let Some(search) = &self.search else {
            return false;
        };
        if !search.handle.is_finished() {
            return false;
        }
        let landed = search.handle.wait().into_result().is_ok();
        if landed {
            self.matches = std::mem::take(
                &mut *search
                    .found
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            );
            self.searched = Some(search.folder.clone());
        }
        self.search = None;
        landed
    }

    /// The plan the current matches describe, relative to `project_dir`.
    #[must_use]
    pub fn plan(&self, project_dir: &Path) -> RelinkPlan {
        RelinkPlan::build(project_dir, &self.matches)
    }

    /// Draws the dialog and returns the plan when the user accepts it.
    ///
    /// Returns `None` while the dialog is closed, still searching, or waiting
    /// to be told what to do. The host applies the returned plan through the
    /// history, where it becomes one undo step.
    pub fn ui(
        &mut self,
        ctx: &egui::Context,
        jobs: &JobService,
        project: &Project,
        project_dir: &Path,
    ) -> Option<RelinkPlan> {
        self.poll();
        if !self.open {
            return None;
        }
        let mut plan = None;
        let mut open = true;
        egui::Window::new("Relink media")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                plan = self.body_ui(ui, jobs, project, project_dir);
            });
        if plan.is_some() {
            open = false;
        }
        if !open {
            self.close();
        }
        plan
    }

    /// The contents of the window.
    fn body_ui(
        &mut self,
        ui: &mut Ui,
        jobs: &JobService,
        project: &Project,
        project_dir: &Path,
    ) -> Option<RelinkPlan> {
        ui.label(match self.targets.len() {
            0 => "Nothing to relink.".to_owned(),
            1 => format!("Relinking {}", self.targets[0].file_name),
            count => format!("Relinking {count} offline items"),
        });

        ui.horizontal(|ui| {
            let single = self.targets.len() == 1;
            let chose = ui
                .add_enabled(single, egui::Button::new("Choose file..."))
                .on_disabled_hover_text("Pick a folder to search when relinking several items")
                .clicked();
            if let Some(file) = chose.then(pick_replacement_file).flatten() {
                self.choose_file(&file);
            }
            let searching = ui.button("Search folder...").clicked();
            if let Some(folder) = searching.then(pick_search_folder).flatten() {
                self.search_folder(jobs, folder);
            }
        });

        if self.is_searching() {
            ui.label(RichText::new("Searching...").weak());
            ui.ctx().request_repaint();
            return None;
        }

        if let Some(folder) = &self.searched {
            ui.label(RichText::new(format!("Searched {}", folder.display())).weak());
        }

        let plan = self.plan(project_dir);
        self.matches_ui(ui, project, &plan);

        let count = plan.len();
        let ready = count > 0;
        let label = if count > 1 {
            format!("Relink {count} items")
        } else {
            "Relink".to_owned()
        };
        let accepted = ui
            .add_enabled(ready, egui::Button::new(label))
            .on_disabled_hover_text("No file inside the project folder was found yet")
            .clicked();
        accepted.then_some(plan)
    }

    /// One row per proposed file, and one per item nothing was found for.
    fn matches_ui(&self, ui: &mut Ui, project: &Project, plan: &RelinkPlan) {
        for found in &self.matches {
            let name = item_name(project, found.media);
            let file = found.file.file_name().map_or_else(
                || found.file.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            );
            ui.label(format!("{name} -> {file} (by {})", found.kind.label()));
        }
        for (media, error) in &plan.rejected {
            ui.label(
                RichText::new(format!("{}: {}", item_name(project, *media), error.message))
                    .color(WARNING),
            );
        }
        for target in &self.targets {
            if !self.matches.iter().any(|found| found.media == target.media) {
                ui.label(RichText::new(format!("{}: not found", target.file_name)).weak());
            }
        }
    }

    /// Reopens the dialog for `targets`, dropping whatever the last one found.
    fn reset(&mut self, targets: Vec<RelinkTarget>) {
        if let Some(search) = &self.search {
            search.handle.cancel();
        }
        self.open = true;
        self.targets = targets;
        self.search = None;
        self.matches.clear();
        self.searched = None;
    }
}

/// The colour a refused file is named in.
const WARNING: egui::Color32 = egui::Color32::from_rgb(220, 120, 90);

/// The display name of `media`, or its file name when the project no longer
/// holds it.
fn item_name(project: &Project, media: MediaId) -> String {
    project
        .media_item(media)
        .map_or_else(|| "media".to_owned(), |item| item.name.clone())
}

/// True when a match came from the bytes rather than the name.
#[must_use]
pub fn is_certain(found: &RelinkMatch) -> bool {
    found.kind == MatchKind::Hash
}

/// Asks the operating system for the file an item should point at.
///
/// Blocking, like every native dialog; it returns `None` when cancelled and on
/// a machine with no desktop portal.
#[must_use]
pub fn pick_replacement_file() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Relink to file")
        .add_filter("All files", &["*"])
        .pick_file()
}

/// Asks the operating system for a folder to search recursively.
#[must_use]
pub fn pick_search_folder() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Search folder for media")
        .pick_folder()
}
