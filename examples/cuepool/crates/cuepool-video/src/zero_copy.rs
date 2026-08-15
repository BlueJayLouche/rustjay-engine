use crate::ZeroCopyPreference;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const INTEROP_FEATURES: wgpu::Features = wgpu::Features::TEXTURE_FORMAT_NV12;
pub(crate) const DIRECT_PATH_POISONED_REASON: &str =
    "zero-copy direct path disabled after a caught panic";
static DIRECT_PATH_POISONED: AtomicBool = AtomicBool::new(false);

pub(crate) fn direct_path_poisoned() -> bool {
    DIRECT_PATH_POISONED.load(Ordering::Acquire)
}

#[derive(Clone)]
pub struct ZeroCopyAvailability {
    reason: Option<Arc<str>>,
    #[cfg(windows)]
    pub(crate) device: Option<Arc<crate::d3d12_zero_copy::InteropDevice>>,
}

impl ZeroCopyAvailability {
    /// Containment only engages where the panic strategy unwinds (dev/test
    /// builds). The release profile keeps `panic = "abort"` deliberately: on
    /// the venue machine a process exit is RECOVERABLE (the site watchdog
    /// relaunches CuePool in seconds), while a silently dead worker thread
    /// under `unwind` would freeze the wall with no external signal. Do not
    /// flip the release profile to `unwind` for this mechanism.
    #[cfg(any(windows, test))]
    pub fn catch_direct_path_panic<T>(operation: impl FnOnce() -> T) -> Result<T, String> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation)).map_err(|payload| {
            let payload = if let Some(message) = payload.downcast_ref::<String>() {
                message.clone()
            } else if let Some(message) = payload.downcast_ref::<&str>() {
                (*message).to_owned()
            } else {
                "non-string panic payload".to_owned()
            };
            if !DIRECT_PATH_POISONED.swap(true, Ordering::AcqRel) {
                log::error!(
                    "Video zero-copy panic: {payload}; disabling the direct path for this process"
                );
            }
            format!("zero-copy direct path panicked: {payload}")
        })
    }

    pub fn required_features(
        adapter: &wgpu::Adapter,
        preference: ZeroCopyPreference,
    ) -> wgpu::Features {
        device_feature_decision(
            preference,
            cfg!(windows),
            adapter.get_info().backend,
            adapter.features(),
            direct_path_poisoned(),
        )
        .0
    }

    pub fn finish(
        adapter: &wgpu::Adapter,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        preference: ZeroCopyPreference,
    ) -> Self {
        let (_, reason) = device_feature_decision(
            preference,
            cfg!(windows),
            adapter.get_info().backend,
            adapter.features(),
            direct_path_poisoned(),
        );
        if let Some(reason) = reason {
            return Self {
                reason: Some(reason.into()),
                #[cfg(windows)]
                device: None,
            };
        }

        #[cfg(windows)]
        {
            match crate::d3d12_zero_copy::InteropDevice::new(adapter, _device, _queue) {
                Ok(device) => Self {
                    reason: None,
                    device: Some(Arc::new(device)),
                },
                Err(reason) => Self {
                    reason: Some(reason.into()),
                    device: None,
                },
            }
        }
        #[cfg(not(windows))]
        unreachable!()
    }

    pub fn fallback_reason(&self) -> Option<&str> {
        if direct_path_poisoned() {
            Some(DIRECT_PATH_POISONED_REASON)
        } else {
            self.reason.as_deref()
        }
    }

    pub fn available(&self) -> bool {
        self.fallback_reason().is_none()
    }

    pub fn declined(reason: impl Into<Arc<str>>) -> Self {
        Self {
            reason: Some(reason.into()),
            #[cfg(windows)]
            device: None,
        }
    }
}

fn device_feature_decision(
    preference: ZeroCopyPreference,
    windows: bool,
    backend: wgpu::Backend,
    available: wgpu::Features,
    poisoned: bool,
) -> (wgpu::Features, Option<String>) {
    if poisoned {
        return (
            wgpu::Features::empty(),
            Some(DIRECT_PATH_POISONED_REASON.into()),
        );
    }
    if !preference.enabled() {
        return (
            wgpu::Features::empty(),
            Some("disabled; set QPLAYER_ZEROCOPY=1 to probe".into()),
        );
    }
    if !windows {
        return (
            wgpu::Features::empty(),
            Some("zero-copy requires Windows".into()),
        );
    }
    if backend != wgpu::Backend::Dx12 {
        return (
            wgpu::Features::empty(),
            Some("zero-copy requires the DX12 backend".into()),
        );
    }
    let missing = INTEROP_FEATURES - available;
    if !missing.is_empty() {
        return (
            wgpu::Features::empty(),
            Some(format!("adapter lacks zero-copy features: {missing:?}")),
        );
    }
    (INTEROP_FEATURES, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_probe_is_opt_in_and_requires_every_feature() {
        let disabled = device_feature_decision(
            ZeroCopyPreference::Disabled,
            true,
            wgpu::Backend::Dx12,
            INTEROP_FEATURES,
            false,
        );
        assert!(disabled.0.is_empty());
        assert!(disabled.1.unwrap().starts_with("disabled"));

        let wrong_backend = device_feature_decision(
            ZeroCopyPreference::Enabled,
            true,
            wgpu::Backend::Vulkan,
            INTEROP_FEATURES,
            false,
        );
        assert!(wrong_backend.0.is_empty());
        assert!(wrong_backend.1.unwrap().contains("DX12 backend"));

        let missing = device_feature_decision(
            ZeroCopyPreference::Enabled,
            true,
            wgpu::Backend::Dx12,
            wgpu::Features::empty(),
            false,
        );
        assert!(missing.0.is_empty());
        assert!(missing.1.unwrap().contains("TEXTURE_FORMAT_NV12"));

        let ready = device_feature_decision(
            ZeroCopyPreference::Enabled,
            true,
            wgpu::Backend::Dx12,
            INTEROP_FEATURES,
            false,
        );
        assert_eq!(ready, (INTEROP_FEATURES, None));
    }

    #[test]
    fn caught_panic_poisons_the_direct_path_and_makes_the_probe_decline() {
        let reason =
            ZeroCopyAvailability::catch_direct_path_panic(|| panic!("test direct-path panic"))
                .unwrap_err();

        let poisoned = device_feature_decision(
            ZeroCopyPreference::Enabled,
            true,
            wgpu::Backend::Dx12,
            INTEROP_FEATURES,
            direct_path_poisoned(),
        );

        assert_eq!(
            reason,
            "zero-copy direct path panicked: test direct-path panic"
        );
        assert!(poisoned.0.is_empty());
        assert_eq!(poisoned.1.as_deref(), Some(DIRECT_PATH_POISONED_REASON));
    }
}
