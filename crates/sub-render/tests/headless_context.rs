//! Bring up a real wgpu device with no window and clear one frame.
//!
//! This is the same code path the app uses, minus eframe, so it proves the
//! device/queue/adapter plumbing works on a machine with only a software
//! adapter (a CI runner). Where no adapter exists at all — a container with
//! no ICD installed — the test reports that and passes rather than failing
//! the build on an environment problem.
//!
//! Every test here shares one context and runs one at a time. The software
//! Vulkan driver CI falls back to (Mesa's lavapipe) has segfaulted when
//! several threads create instances and drive devices at once, so the driver
//! is only ever touched by one thread.

use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use sub_render::{RenderContext, RenderError};

/// Held for the length of a test: only one test may talk to the driver.
static DRIVER: Mutex<()> = Mutex::new(());
/// The one context, built under `DRIVER`. `None` means no usable adapter.
static CONTEXT: OnceLock<Option<RenderContext>> = OnceLock::new();

/// Exclusive use of the shared context, or `None` when this machine has no
/// usable adapter. The guard must outlive every use of the context.
fn context_or_skip() -> Option<(MutexGuard<'static, ()>, RenderContext)> {
    let guard = DRIVER.lock().unwrap_or_else(PoisonError::into_inner);
    let context = CONTEXT.get_or_init(|| match RenderContext::headless() {
        Ok(context) => Some(context),
        Err(RenderError::NoAdapter { backends }) => {
            eprintln!("skipping: no wgpu adapter for backends [{backends}]");
            None
        }
        Err(error) => panic!("[{}] {error}", error.code()),
    });
    context.clone().map(|context| (guard, context))
}

#[test]
fn a_headless_context_reports_its_adapter() {
    let Some((_driver, context)) = context_or_skip() else {
        return;
    };

    let description = context.describe();
    assert!(
        description.contains(context.backend_label()),
        "adapter description {description:?} should name the backend"
    );
    assert!(
        description.contains(context.device_type_label()),
        "adapter description {description:?} should name the device type"
    );
    assert_eq!(context.adapter_info().backend, context.backend());
    eprintln!("adapter: {description}");
}

/// Clear one texture, creating it through `texture_context` and encoding and
/// submitting through `encoder_context`. Passing a clone as the second
/// context proves both handles drive the one device: wgpu rejects a resource
/// that belongs to another device.
fn clear_a_frame(texture_context: &RenderContext, encoder_context: &RenderContext) {
    let device = texture_context.device();
    let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("empty frame"),
        size: wgpu::Extent3d {
            width: 64,
            height: 32,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());

    let mut encoder =
        encoder_context
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("empty frame"),
            });
    drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("clear"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    }));
    encoder_context.queue().submit([encoder.finish()]);
    encoder_context
        .device()
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("the empty frame should finish");

    let error = pollster::block_on(error_scope.pop());
    assert!(error.is_none(), "clearing a frame raised {error:?}");
}

#[test]
fn a_headless_context_renders_an_empty_frame() {
    let Some((_driver, context)) = context_or_skip() else {
        return;
    };
    clear_a_frame(&context, &context);
}

#[test]
fn cloning_a_context_keeps_the_same_device() {
    let Some((_driver, context)) = context_or_skip() else {
        return;
    };

    let clone = context.clone();
    assert_eq!(clone.adapter_info(), context.adapter_info());
    // Sharing the device is the whole point of the shared render context: a
    // texture made through one handle must be usable through the other.
    clear_a_frame(&context, &clone);
}
