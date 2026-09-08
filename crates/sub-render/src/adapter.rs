//! Adapter ranking and selection.
//!
//! The UI and the compositor share one device (docs/PLAN.md §3), so the
//! adapter is chosen once, by these functions, and both eframe and any
//! headless context use the same rule: prefer a discrete GPU, then an
//! integrated one, then a virtual one, and only fall back to a software
//! (CPU) adapter when nothing else exists. CI runners have no GPU, so that
//! last rung is what keeps the app startable there.

use wgpu::{Adapter, AdapterInfo, Backend, DeviceType};

/// How much a given adapter is wanted, highest first.
///
/// The numeric values are an implementation detail; compare `AdapterRank`
/// values with `Ord` rather than casting them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AdapterRank {
    /// CPU / software rendering. Correct but slow; the CI fallback.
    Software,
    /// Unknown hardware.
    Other,
    /// A virtualised or hosted GPU.
    Virtual,
    /// An integrated GPU sharing memory with the CPU.
    Integrated,
    /// A discrete GPU with its own memory. What we want.
    Discrete,
}

/// Rank one adapter's device type.
pub fn adapter_rank(device_type: DeviceType) -> AdapterRank {
    match device_type {
        DeviceType::DiscreteGpu => AdapterRank::Discrete,
        DeviceType::IntegratedGpu => AdapterRank::Integrated,
        DeviceType::VirtualGpu => AdapterRank::Virtual,
        DeviceType::Cpu => AdapterRank::Software,
        DeviceType::Other => AdapterRank::Other,
    }
}

/// Index of the most preferred adapter in `device_types`, or `None` when the
/// slice is empty.
///
/// Ties keep enumeration order, so selection is deterministic on a machine
/// with two identical GPUs.
pub fn best_adapter_index(device_types: &[DeviceType]) -> Option<usize> {
    device_types
        .iter()
        .enumerate()
        .max_by_key(|(index, device_type)| {
            // `max_by_key` returns the last maximum, so invert the index to
            // keep the first adapter of an equally ranked pair.
            (adapter_rank(**device_type), std::cmp::Reverse(*index))
        })
        .map(|(index, _)| index)
}

/// Pick the most preferred adapter out of an enumerated list.
pub fn select_adapter(adapters: &[Adapter]) -> Option<&Adapter> {
    let device_types: Vec<DeviceType> = adapters
        .iter()
        .map(|adapter| adapter.get_info().device_type)
        .collect();
    best_adapter_index(&device_types).map(|index| &adapters[index])
}

/// The name Subordinate logs for a wgpu backend.
///
/// These are the spellings used in docs/PLAN.md (Vulkan, D3D12, Metal), not
/// wgpu's internal short names.
pub fn backend_label(backend: Backend) -> &'static str {
    match backend {
        Backend::Vulkan => "Vulkan",
        Backend::Metal => "Metal",
        Backend::Dx12 => "D3D12",
        Backend::Gl => "OpenGL",
        Backend::BrowserWebGpu => "WebGPU",
        Backend::Noop => "Noop",
    }
}

/// The word Subordinate logs for a device type.
pub fn device_type_label(device_type: DeviceType) -> &'static str {
    match device_type {
        DeviceType::DiscreteGpu => "discrete GPU",
        DeviceType::IntegratedGpu => "integrated GPU",
        DeviceType::VirtualGpu => "virtual GPU",
        DeviceType::Cpu => "software",
        DeviceType::Other => "unknown",
    }
}

/// One-line description of an adapter, for logs and the about box.
///
/// Example: `NVIDIA GeForce RTX 4070 (discrete GPU, Vulkan, driver 550.90)`.
pub fn describe_adapter(info: &AdapterInfo) -> String {
    let mut description = format!(
        "{} ({}, {}",
        info.name,
        device_type_label(info.device_type),
        backend_label(info.backend)
    );
    if !info.driver.is_empty() {
        description.push_str(", driver ");
        description.push_str(&info.driver);
        if !info.driver_info.is_empty() {
            description.push(' ');
            description.push_str(&info.driver_info);
        }
    }
    description.push(')');
    description
}

#[cfg(test)]
mod tests {
    use super::{AdapterRank, adapter_rank, backend_label, best_adapter_index, device_type_label};
    use wgpu::{Backend, DeviceType};

    #[test]
    fn discrete_outranks_everything_else() {
        assert!(adapter_rank(DeviceType::DiscreteGpu) > adapter_rank(DeviceType::IntegratedGpu));
        assert!(adapter_rank(DeviceType::IntegratedGpu) > adapter_rank(DeviceType::VirtualGpu));
        assert!(adapter_rank(DeviceType::VirtualGpu) > adapter_rank(DeviceType::Other));
        assert!(adapter_rank(DeviceType::Other) > adapter_rank(DeviceType::Cpu));
        assert_eq!(adapter_rank(DeviceType::Cpu), AdapterRank::Software);
    }

    #[test]
    fn no_adapters_means_no_choice() {
        assert_eq!(best_adapter_index(&[]), None);
    }

    #[test]
    fn a_discrete_gpu_is_chosen_over_an_integrated_one() {
        let adapters = [
            DeviceType::IntegratedGpu,
            DeviceType::Cpu,
            DeviceType::DiscreteGpu,
        ];
        assert_eq!(best_adapter_index(&adapters), Some(2));
    }

    #[test]
    fn software_is_used_only_when_it_is_all_there_is() {
        assert_eq!(best_adapter_index(&[DeviceType::Cpu]), Some(0));
        assert_eq!(
            best_adapter_index(&[DeviceType::Cpu, DeviceType::IntegratedGpu]),
            Some(1)
        );
    }

    #[test]
    fn equal_ranks_keep_enumeration_order() {
        let adapters = [DeviceType::DiscreteGpu, DeviceType::DiscreteGpu];
        assert_eq!(best_adapter_index(&adapters), Some(0));
    }

    #[test]
    fn backends_use_the_plan_spellings() {
        assert_eq!(backend_label(Backend::Vulkan), "Vulkan");
        assert_eq!(backend_label(Backend::Dx12), "D3D12");
        assert_eq!(backend_label(Backend::Metal), "Metal");
        assert_eq!(backend_label(Backend::Gl), "OpenGL");
    }

    #[test]
    fn device_types_have_labels() {
        assert_eq!(device_type_label(DeviceType::DiscreteGpu), "discrete GPU");
        assert_eq!(device_type_label(DeviceType::Cpu), "software");
    }
}
