//! The render context shared by the UI and the compositor.
//!
//! docs/PLAN.md §3 chose egui/eframe partly because it renders through wgpu:
//! the compositor writes preview frames into a texture the UI samples
//! directly, with no readback and no copy. That only works if both sides hold
//! the *same* `wgpu::Device` and `wgpu::Queue`, so a `RenderContext` is
//! created once — normally from eframe's `RenderState` — and cloned to
//! everyone who needs it.

use crate::adapter::{backend_label, describe_adapter, device_type_label, select_adapter};
use crate::error::RenderError;
use wgpu::{
    Adapter, AdapterInfo, Backend, Backends, Device, DeviceDescriptor, DeviceType, Instance,
    InstanceDescriptor, Queue,
};

/// A wgpu device shared between the egui UI and the compositor.
///
/// Cloning is cheap: `Device` and `Queue` are reference-counted handles to the
/// one device, so every clone talks to the same GPU queue.
#[derive(Debug, Clone)]
pub struct RenderContext {
    device: Device,
    queue: Queue,
    adapter_info: AdapterInfo,
}

impl RenderContext {
    /// Wrap an already-created device, queue and adapter.
    ///
    /// This is how the UI hands eframe's `RenderState` to the compositor.
    pub fn new(device: Device, queue: Queue, adapter_info: AdapterInfo) -> Self {
        Self {
            device,
            queue,
            adapter_info,
        }
    }

    /// The shared device. Textures created here are usable by the UI.
    pub fn device(&self) -> &Device {
        &self.device
    }

    /// The shared queue. Submissions are ordered with the UI's own.
    pub fn queue(&self) -> &Queue {
        &self.queue
    }

    /// Adapter the device was created from.
    pub fn adapter_info(&self) -> &AdapterInfo {
        &self.adapter_info
    }

    /// Graphics backend in use.
    pub fn backend(&self) -> Backend {
        self.adapter_info.backend
    }

    /// Kind of GPU (or software renderer) in use.
    pub fn device_type(&self) -> DeviceType {
        self.adapter_info.device_type
    }

    /// `true` when rendering falls back to the CPU, as on a CI runner.
    pub fn is_software(&self) -> bool {
        self.adapter_info.device_type == DeviceType::Cpu
    }

    /// One-line description for logs and the about box.
    pub fn describe(&self) -> String {
        describe_adapter(&self.adapter_info)
    }

    /// The backend name as logged: `Vulkan`, `D3D12`, `Metal`, ...
    pub fn backend_label(&self) -> &'static str {
        backend_label(self.adapter_info.backend)
    }

    /// The device-type name as logged: `discrete GPU`, `software`, ...
    pub fn device_type_label(&self) -> &'static str {
        device_type_label(self.adapter_info.device_type)
    }

    /// Create a context with no window attached.
    ///
    /// Used by tests, by the CLI renderer and by export, none of which open a
    /// window. Adapter preference is the same as the UI's: discrete first,
    /// software last, and if no adapter is enumerated at all a fallback
    /// (software) adapter is requested explicitly.
    ///
    /// # Errors
    ///
    /// [`RenderError::NoAdapter`] when the platform offers no adapter at all,
    /// and [`RenderError::DeviceRequestFailed`] when one is found but will not
    /// create a device.
    pub fn headless() -> Result<Self, RenderError> {
        Self::headless_with_backends(Backends::from_env().unwrap_or(Backends::PRIMARY))
    }

    /// [`RenderContext::headless`] restricted to particular backends.
    ///
    /// # Errors
    ///
    /// As [`RenderContext::headless`].
    pub fn headless_with_backends(backends: Backends) -> Result<Self, RenderError> {
        let instance = Instance::new(InstanceDescriptor {
            backends,
            ..InstanceDescriptor::new_without_display_handle_from_env()
        });
        let adapters = pollster::block_on(instance.enumerate_adapters(backends));
        let adapter = select_adapter(&adapters)
            .cloned()
            .or_else(|| request_fallback_adapter(&instance))
            .ok_or_else(|| RenderError::NoAdapter {
                backends: format!("{backends:?}"),
            })?;
        Self::from_adapter(&adapter)
    }

    /// Request a device from `adapter` and wrap it.
    ///
    /// # Errors
    ///
    /// [`RenderError::DeviceRequestFailed`] if the adapter will not create a
    /// device with our default limits.
    pub fn from_adapter(adapter: &Adapter) -> Result<Self, RenderError> {
        let adapter_info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: Some("subordinate device"),
            required_features: wgpu::Features::empty(),
            required_limits: adapter.limits(),
            memory_hints: wgpu::MemoryHints::default(),
            trace: wgpu::Trace::Off,
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
        }))
        .map_err(|error| RenderError::DeviceRequestFailed {
            adapter: describe_adapter(&adapter_info),
            reason: error.to_string(),
        })?;
        Ok(Self::new(device, queue, adapter_info))
    }
}

/// Ask for a software adapter when enumeration turned up nothing.
fn request_fallback_adapter(instance: &Instance) -> Option<Adapter> {
    pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::None,
        force_fallback_adapter: true,
        compatible_surface: None,
        apply_limit_buckets: false,
    }))
    .ok()
}
