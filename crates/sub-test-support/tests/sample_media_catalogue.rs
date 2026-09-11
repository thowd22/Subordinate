//! Holds the sample-media fetch scripts to what CI and the docs promise.
//!
//! The three CC0 clips `examples/sample-project/demo.sub` plays are served
//! from this project's own `sample-media-v1` release rather than from
//! Wikimedia Commons, where they were first published: concurrent CI runs off
//! shared runner egress were answered with HTTP 429, and no run should depend
//! on a third-party site (TASK-132). The Commons URLs stay in the catalogue
//! for provenance.
//!
//! Nothing here touches the network. It reads the two scripts, the workflow
//! and the sample project README as text and checks they still agree: the
//! catalogues match file for file, every download goes to the release, every
//! entry keeps its provenance, the credits are complete, and the CI cache is
//! keyed on both scripts so a warm run downloads nothing.

use std::path::{Path, PathBuf};

/// Release the media is served from. Bumping it means republishing the assets.
const RELEASE_TAG: &str = "sample-media-v1";
/// Base the download URLs are built from, minus the trailing file name.
const RELEASE_BASE: &str = "https://github.com/thowd22/Subordinate/releases/download";
/// Host the files were originally published on. Provenance only.
const UPSTREAM_HOST: &str = "upload.wikimedia.org";

/// One catalogue record, as `scripts/get-sample-media.sh` spells it.
struct Media {
    name: String,
    bytes: u64,
    sha256: String,
    origin: String,
    source: String,
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root is two levels above this crate")
}

fn read(relative: &str) -> String {
    let path = repo_root().join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

/// Parses the tab-separated catalogue out of the shell script. Catalogue lines
/// are the only tab-separated ones in it, and every field is required.
fn shell_catalogue(script: &str) -> Vec<Media> {
    script
        .lines()
        .filter(|line| line.contains('\t'))
        .map(|line| {
            let fields: Vec<&str> = line.split('\t').collect();
            assert_eq!(
                fields.len(),
                7,
                "catalogue line has {} fields, expected 7: {line}",
                fields.len()
            );
            Media {
                name: fields[0].to_owned(),
                bytes: fields[1]
                    .parse()
                    .unwrap_or_else(|e| panic!("byte count {:?} is not a number: {e}", fields[1])),
                sha256: fields[2].to_owned(),
                origin: fields[3].to_owned(),
                source: fields[6].to_owned(),
            }
        })
        .collect()
}

#[test]
fn the_shell_catalogue_is_well_formed() {
    let media = shell_catalogue(&read("scripts/get-sample-media.sh"));
    assert_eq!(media.len(), 3, "the sample project plays three clips");
    for m in &media {
        assert!(
            Path::new(&m.name)
                .extension()
                .is_some_and(|ext| ext == "webm")
                && !m.name.contains('/'),
            "{:?} is not a plain file name",
            m.name
        );
        assert!(
            m.sha256.len() == 64 && m.sha256.chars().all(|c| c.is_ascii_hexdigit()),
            "{} is not pinned by a SHA-256: {:?}",
            m.name,
            m.sha256
        );
        assert!(m.bytes > 0, "{} is pinned to zero bytes", m.name);
    }
}

#[test]
fn every_download_comes_from_the_project_release() {
    let script = read("scripts/get-sample-media.sh");
    let media = shell_catalogue(&script);

    // The default URL is release_base/name, and release_base is this project's
    // release download endpoint at the pinned tag.
    assert!(
        script.contains(&format!("release_tag=\"{RELEASE_TAG}\"")),
        "the shell script does not pin the release tag {RELEASE_TAG}"
    );
    assert!(
        script.contains(&format!("release_base=\"{RELEASE_BASE}/$release_tag\"")),
        "the shell script does not build its URLs from the release download endpoint"
    );
    assert!(
        script.contains("echo \"$release_base/$name\""),
        "the shell script's default download URL is not a release asset"
    );
    assert!(
        script.contains("checksums_url=\"$release_base/SHA256SUMS\""),
        "the shell script does not verify against the release SHA256SUMS"
    );

    // Wikimedia is provenance only: it may appear as an origin or a Commons
    // page, and is reached only under the opt-in --upstream switch.
    for m in &media {
        assert!(
            m.origin.contains(UPSTREAM_HOST),
            "{} lost its upstream origin URL",
            m.name
        );
        assert!(
            m.source
                .starts_with("https://commons.wikimedia.org/wiki/File:"),
            "{} lost its Commons provenance page: {:?}",
            m.name,
            m.source
        );
    }
    assert!(
        script.contains("--upstream"),
        "the opt-in upstream switch is gone, so the origin URLs are never exercised"
    );
}

#[test]
fn the_powershell_twin_pins_the_same_files() {
    let shell = read("scripts/get-sample-media.sh");
    let ps = read("scripts/get-sample-media.ps1");
    for m in shell_catalogue(&shell) {
        assert!(ps.contains(&m.name), "{} is missing from the .ps1", m.name);
        assert!(
            ps.contains(&m.sha256),
            "{} is pinned to a different SHA-256 in the .ps1",
            m.name
        );
        assert!(
            ps.contains(&format!("{}L", m.bytes)),
            "{} is pinned to a different byte count in the .ps1",
            m.name
        );
        assert!(
            ps.contains(&m.origin),
            "{} lost its upstream origin URL in the .ps1",
            m.name
        );
    }
    assert!(
        ps.contains(&format!("$releaseTag = '{RELEASE_TAG}'")),
        "the .ps1 does not pin the release tag {RELEASE_TAG}"
    );
    assert!(
        ps.contains("\"$releaseBase/$($Media.name)\""),
        "the .ps1's default download URL is not a release asset"
    );
    assert!(
        ps.contains("$checksumsUrl = \"$releaseBase/SHA256SUMS\""),
        "the .ps1 does not verify against the release SHA256SUMS"
    );
}

#[test]
fn ci_caches_the_media_and_never_contacts_wikimedia() {
    let workflow = read(".github/workflows/ci.yml");
    assert!(
        workflow.contains(
            "sample-media-${{ runner.os }}-${{ hashFiles('scripts/get-sample-media.sh', \
             'scripts/get-sample-media.ps1') }}"
        ),
        "the sample media cache is not keyed on both fetch scripts"
    );
    assert!(
        workflow.contains("run: ./scripts/get-sample-media.sh\n"),
        "the fetch step no longer runs the script plainly"
    );
    assert!(
        !workflow.contains("--upstream"),
        "CI opted into the upstream Wikimedia URLs"
    );
    assert!(
        !workflow.contains(UPSTREAM_HOST),
        "the workflow reaches {UPSTREAM_HOST} directly"
    );
}

#[test]
fn the_readme_credits_every_file_and_names_the_release() {
    let readme = read("examples/sample-project/README.md");
    for m in shell_catalogue(&read("scripts/get-sample-media.sh")) {
        assert!(
            readme.contains(&format!("media/{}", m.name)),
            "{} is not credited in examples/sample-project/README.md",
            m.name
        );
        assert!(
            readme.contains(&m.source),
            "{}'s provenance page is not recorded in the README",
            m.name
        );
    }
    assert!(
        readme.contains("CC0"),
        "the README no longer records the licence"
    );
    assert!(
        readme.contains(RELEASE_TAG),
        "the README does not say which release the media is served from"
    );
}
