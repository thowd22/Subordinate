//! How much memory the harness held while it measured.
//!
//! Phase 5's exit criterion asks that editing an hour-long source with proxies
//! stays under a documented budget (docs/PLAN.md §8), so the run reports the
//! peak resident set size of its own process: the decoder's buffers, the frame
//! it hands over, the upload staging and whatever the proxy transcode held,
//! all of it together.
//!
//! Only Linux is measured. The kernel keeps the high-water mark for us in
//! `/proc/self/status` as `VmHWM`, which needs no sampling thread and cannot
//! miss a spike between samples. On every other platform the peak is reported
//! as unmeasured rather than guessed at, and the criterion is recorded as not
//! proven there.

/// Peak resident set size of this process in bytes, or `None` where the
/// platform does not report one.
#[cfg(target_os = "linux")]
pub fn peak_rss_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    parse_vm_hwm(&status)
}

/// Peak resident set size of this process in bytes, or `None` where the
/// platform does not report one.
#[cfg(not(target_os = "linux"))]
pub fn peak_rss_bytes() -> Option<u64> {
    None
}

/// The `VmHWM` line of a `/proc/self/status` document, in bytes.
///
/// The kernel writes the value in kibibytes; anything else in the line's unit
/// column is refused rather than misread.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_vm_hwm(status: &str) -> Option<u64> {
    let line = status
        .lines()
        .find(|line| line.starts_with("VmHWM:"))?
        .trim_start_matches("VmHWM:");
    let mut fields = line.split_whitespace();
    let kibibytes: u64 = fields.next()?.parse().ok()?;
    if fields.next()? != "kB" {
        return None;
    }
    kibibytes.checked_mul(1024)
}

#[cfg(test)]
mod tests {
    use super::parse_vm_hwm;

    const STATUS: &str = "Name:\tsubordinate-bench\nVmPeak:\t 2097152 kB\nVmHWM:\t  524288 kB\nVmRSS:\t  131072 kB\n";

    #[test]
    fn the_high_water_mark_is_read_in_bytes() {
        assert_eq!(parse_vm_hwm(STATUS), Some(524_288 * 1024));
    }

    #[test]
    fn a_document_without_the_line_reports_nothing() {
        assert_eq!(parse_vm_hwm("Name:\tx\nVmRSS:\t 4 kB\n"), None);
        assert_eq!(parse_vm_hwm(""), None);
    }

    #[test]
    fn an_unexpected_unit_is_refused_rather_than_misread() {
        assert_eq!(parse_vm_hwm("VmHWM:\t 512 MB\n"), None);
        assert_eq!(parse_vm_hwm("VmHWM:\t lots kB\n"), None);
        assert_eq!(parse_vm_hwm("VmHWM:\t 512\n"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn this_process_reports_a_peak() {
        let peak = super::peak_rss_bytes().expect("linux reports a peak");
        assert!(peak > 0, "a running process has held some memory");
    }
}
