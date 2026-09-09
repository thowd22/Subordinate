//! Asks the real audio host what it offers.
//!
//! This is the one test that touches cpal's platform backend — ALSA or
//! PipeWire on Linux, WASAPI on Windows, `CoreAudio` on macOS. A build machine
//! may have no sound card at all, so the test asserts on the *shape* of the
//! answer rather than on there being a device: whatever comes back must be
//! describable, and every device must be able to negotiate a format for a
//! 48 kHz stereo sequence. Opening a stream needs real hardware and is a
//! manual check.

use sub_audio::{CpalBackend, OutputBackend, codes};

#[test]
fn the_host_describes_whatever_output_devices_it_has() {
    let backend = CpalBackend::new();
    let devices = match backend.devices() {
        Ok(devices) => devices,
        Err(error) => {
            // A machine with no sound server says so; that is not a failure.
            assert_eq!(
                error.code,
                codes::DEVICE_UNAVAILABLE,
                "an unusable host should be reported as unavailable: {error}"
            );
            return;
        }
    };
    let defaults = devices.iter().filter(|device| device.is_default()).count();
    assert!(defaults <= 1, "at most one device is the host's default");
    if let Some(first) = devices.first() {
        assert!(
            devices[1..].iter().all(|device| !device.is_default()),
            "the default device is listed first"
        );
        let _ = first;
    }
    for device in &devices {
        assert!(!device.id().is_empty(), "a device needs a stable id");
        assert!(!device.name().is_empty(), "a device needs a name");
        if device.formats().is_empty() {
            // A device that advertises only formats this build cannot write.
            let error = device.negotiate(48_000, 2).expect_err("no usable format");
            assert_eq!(error.code, codes::FORMAT_UNSUPPORTED);
            continue;
        }
        let format = device
            .negotiate(48_000, 2)
            .unwrap_or_else(|error| panic!("{}: {error}", device.name()));
        assert_eq!(format.source_sample_rate(), 48_000);
        assert!(format.sample_rate() > 0);
        assert!(format.channels() > 0);
    }
}
