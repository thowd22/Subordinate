//! The headless half of the measurement: what one NV12 frame costs to get
//! onto the GPU and converted, with no decoder and no window involved.
//!
//! Splitting this out is what makes the spike measurable on a machine that
//! cannot decode (a CI runner, a container with no VA-API): the upload path
//! here is byte-for-byte the one `play` uses, so its numbers stand on their
//! own and can be compared against a GPU box later.

use std::sync::Arc;
use std::time::{Duration, Instant};

use sub_core::{ResultExt as _, SubError, SubResult};
use sub_render::RenderContext;

use crate::codes;
use crate::decode::Decoder;
use crate::frames::{Durations, FrameSlot};
use crate::nv12::{Nv12Converter, Nv12Geometry, synthetic_frame};

/// What one benchmark run measured.
#[derive(Debug)]
pub struct BenchReport {
    /// The picture size measured.
    pub geometry: Nv12Geometry,
    /// The adapter the work ran on, as logged.
    pub adapter: String,
    /// Whether that adapter is a CPU renderer, which caps how much the
    /// numbers say about real hardware.
    pub software: bool,
    /// Wall time of `write_texture` for both planes, per frame.
    pub upload: Durations,
    /// Wall time of upload plus the conversion pass, waited to completion.
    pub upload_and_convert: Durations,
}

impl BenchReport {
    /// Effective upload throughput in mebibytes per second, from the mean.
    ///
    /// Integer maths: bytes and nanoseconds only.
    pub fn upload_mib_per_second(&self) -> Option<u64> {
        let mean = self.upload.mean_nanos()?;
        if mean == 0 {
            return None;
        }
        let bytes = self.geometry.payload_len() as u128;
        u64::try_from(bytes * 1_000_000_000 / (u128::from(mean) * 1024 * 1024)).ok()
    }

    /// The frame rate the upload-and-convert step alone could sustain.
    pub fn sustainable_fps(&self) -> Option<u64> {
        let mean = self.upload_and_convert.mean_nanos()?;
        (mean > 0).then(|| 1_000_000_000 / mean)
    }

    /// The report as the lines the spike prints and the findings quote.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![
            format!(
                "picture      {}x{} NV12, {} bytes of payload per frame",
                self.geometry.width(),
                self.geometry.height(),
                self.geometry.payload_len()
            ),
            format!(
                "adapter      {}{}",
                self.adapter,
                if self.software {
                    " [software: treat every number as an upper bound]"
                } else {
                    ""
                }
            ),
            format!("upload       {}", self.upload.summary_micros()),
            format!("upload+conv  {}", self.upload_and_convert.summary_micros()),
        ];
        if let Some(throughput) = self.upload_mib_per_second() {
            lines.push(format!("throughput   {throughput} MiB/s of NV12 payload"));
        }
        if let Some(fps) = self.sustainable_fps() {
            lines.push(format!(
                "headroom     {fps} frames/s from upload+convert alone"
            ));
        }
        lines
    }
}

/// Upload and convert `frames` synthesised pictures of `geometry`, timing
/// each one.
///
/// Every frame differs, so no driver can elide the copy, and the GPU is
/// waited on per frame so the number is a real completion time rather than a
/// queue submission.
///
/// # Errors
///
/// [`codes::NO_DEVICE`] when the machine has no usable wgpu adapter, and
/// whatever [`Nv12Converter::upload`] reports for a malformed frame.
pub fn run(geometry: Nv12Geometry, frames: u32) -> SubResult<BenchReport> {
    let context = RenderContext::headless()
        .sub_context(codes::NO_DEVICE, "no wgpu adapter for the benchmark")?;
    let device = context.device();
    let queue = context.queue();
    let converter = Nv12Converter::new(device, geometry);

    let mut upload = Durations::new();
    let mut upload_and_convert = Durations::new();

    // One warm-up frame: the first submission pays for shader compilation and
    // the first allocation of the staging belt, which is not a per-frame cost.
    let (y, uv) = synthetic_frame(geometry, 0);
    converter.submit_frame(device, queue, &y, &uv)?;
    wait_for_gpu(&context)?;

    for index in 0..frames {
        let phase = u8::try_from(index % 251).unwrap_or(0);
        let (y, uv) = synthetic_frame(geometry, phase);

        let started = Instant::now();
        converter.upload(queue, &y, &uv)?;
        let uploaded = Instant::now();

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("spike bench frame"),
        });
        converter.encode_conversion(&mut encoder);
        queue.submit([encoder.finish()]);
        wait_for_gpu(&context)?;
        let finished = Instant::now();

        upload.push_nanos(nanos_between(started, uploaded));
        upload_and_convert.push_nanos(nanos_between(started, finished));
    }

    Ok(BenchReport {
        geometry,
        adapter: context.describe(),
        software: context.is_software(),
        upload,
        upload_and_convert,
    })
}

/// What one end-to-end decode run measured.
#[derive(Debug)]
pub struct DecodeReport {
    /// The picture size the file decoded to.
    pub geometry: Nv12Geometry,
    /// The decoder element the pipeline instantiated.
    pub decoder: String,
    /// The adapter the upload ran on.
    pub adapter: String,
    /// Frames the appsink delivered.
    pub delivered: u64,
    /// Frames the newest-wins slot discarded.
    pub dropped: u64,
    /// Upload plus conversion, per displayed frame.
    pub upload_and_convert: Durations,
    /// Appsink hand-off to converted-and-on-the-GPU, per displayed frame.
    /// This is decode-to-display minus the swapchain present.
    pub latency: Durations,
}

impl DecodeReport {
    /// The report as printable lines.
    pub fn lines(&self) -> Vec<String> {
        vec![
            format!(
                "picture      {}x{} NV12 (luma stride {}, chroma stride {})",
                self.geometry.width(),
                self.geometry.height(),
                self.geometry.y_stride(),
                self.geometry.uv_stride()
            ),
            format!("decoder      {}", self.decoder),
            format!("adapter      {}", self.adapter),
            format!(
                "frames       delivered {}, dropped by the newest-wins slot {}",
                self.delivered, self.dropped
            ),
            format!("upload+conv  {}", self.upload_and_convert.summary_micros()),
            format!("latency      {}", self.latency.summary_micros()),
        ]
    }
}

/// Decode `uri` and push every frame through the real upload path, with no
/// window in the way.
///
/// This is the measurement the windowed spike cannot take on a machine with
/// no compositor: it covers appsink hand-off, the plane copy, both
/// `write_texture` calls and the conversion pass, waited to GPU completion.
/// Only the swapchain present is missing.
///
/// # Errors
///
/// Whatever the decoder or the upload path reports, plus
/// [`codes::PIPELINE_FAILED`] when no frame arrives within `timeout`.
pub fn run_decode(uri: &str, frames: u32, timeout: Duration) -> SubResult<DecodeReport> {
    let context = RenderContext::headless()
        .sub_context(codes::NO_DEVICE, "no wgpu adapter for the decode benchmark")?;
    let slot = Arc::new(FrameSlot::new());
    let decoder = Decoder::start(uri, Arc::clone(&slot))?;

    let mut converter: Option<Nv12Converter> = None;
    let mut upload_and_convert = Durations::new();
    let mut latency = Durations::new();
    let mut geometry = None;
    let deadline = Instant::now() + timeout;

    while upload_and_convert.len() < frames as usize {
        if let Some(error) = decoder.take_error() {
            return Err(error);
        }
        let Some(frame) = slot.take() else {
            if Instant::now() > deadline {
                break;
            }
            std::thread::yield_now();
            continue;
        };
        let converter = match &converter {
            Some(existing) if existing.geometry() == frame.geometry => existing,
            _ => {
                geometry = Some(frame.geometry);
                converter.insert(Nv12Converter::new(context.device(), frame.geometry))
            }
        };
        let started = Instant::now();
        converter.submit_frame(context.device(), context.queue(), &frame.y, &frame.uv)?;
        wait_for_gpu(&context)?;
        let finished = Instant::now();
        upload_and_convert.push_nanos(nanos_between(started, finished));
        latency.push_nanos(nanos_between(frame.arrived, finished));
    }

    let geometry = geometry.ok_or_else(|| {
        SubError::new(
            codes::PIPELINE_FAILED,
            format!("no frame arrived from {uri} within {timeout:?}"),
        )
    })?;
    Ok(DecodeReport {
        geometry,
        decoder: decoder
            .chosen_decoder()
            .unwrap_or_else(|| "unknown".to_owned()),
        adapter: context.describe(),
        delivered: decoder.stats().delivered(),
        dropped: decoder.stats().dropped(),
        upload_and_convert,
        latency,
    })
}

/// Block until every submitted command has completed.
///
/// # Errors
///
/// [`codes::GPU_LOST`] when the device stops responding.
fn wait_for_gpu(context: &RenderContext) -> SubResult<()> {
    context
        .device()
        .poll(wgpu::PollType::wait_indefinitely())
        .sub_context(codes::GPU_LOST, "the GPU did not finish the frame")?;
    Ok(())
}

/// Nanoseconds between two instants, saturating rather than panicking.
fn nanos_between(start: Instant, end: Instant) -> u64 {
    u64::try_from(end.saturating_duration_since(start).as_nanos()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::BenchReport;
    use crate::frames::Durations;
    use crate::nv12::Nv12Geometry;

    fn report(upload_nanos: u64, convert_nanos: u64) -> BenchReport {
        let mut upload = Durations::new();
        upload.push_nanos(upload_nanos);
        let mut upload_and_convert = Durations::new();
        upload_and_convert.push_nanos(convert_nanos);
        BenchReport {
            geometry: Nv12Geometry::packed(3840, 2160).expect("4K is a valid picture"),
            adapter: "llvmpipe (software, Vulkan)".to_owned(),
            software: true,
            upload,
            upload_and_convert,
        }
    }

    #[test]
    fn throughput_is_payload_over_mean_upload_time() {
        // 12 441 600 bytes in exactly 12 ms is 11.86 MiB/ms -> ~989 MiB/s.
        let report = report(12_000_000, 20_000_000);
        let throughput = report.upload_mib_per_second().expect("a mean was recorded");
        assert!(
            (980..=1000).contains(&throughput),
            "unexpected throughput {throughput} MiB/s"
        );
    }

    #[test]
    fn sustainable_fps_comes_from_upload_plus_conversion() {
        let report = report(12_000_000, 20_000_000);
        assert_eq!(report.sustainable_fps(), Some(50));
    }

    #[test]
    fn a_zero_measurement_reports_no_rate_instead_of_dividing_by_zero() {
        let report = report(0, 0);
        assert_eq!(report.upload_mib_per_second(), None);
        assert_eq!(report.sustainable_fps(), None);
    }

    #[test]
    fn a_software_adapter_is_called_out_in_the_report() {
        let lines = report(1_000, 2_000).lines().join("\n");
        assert!(lines.contains("software"), "{lines}");
        assert!(lines.contains("3840x2160"), "{lines}");
    }
}
