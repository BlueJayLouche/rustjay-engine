use crate::ZeroCopyPreference;
use std::sync::Arc;

const INTEROP_FEATURES: wgpu::Features = wgpu::Features::TEXTURE_FORMAT_NV12
    .union(wgpu::Features::VULKAN_EXTERNAL_MEMORY_WIN32)
    .union(wgpu::Features::VULKAN_WIN32_KEYED_MUTEX);

#[derive(Clone)]
pub struct ZeroCopyAvailability {
    reason: Option<Arc<str>>,
    #[cfg(windows)]
    pub(crate) device: Option<Arc<crate::d3d11_zero_copy::InteropDevice>>,
}

impl ZeroCopyAvailability {
    pub fn required_features(
        adapter: &wgpu::Adapter,
        preference: ZeroCopyPreference,
    ) -> wgpu::Features {
        device_feature_decision(
            preference,
            cfg!(windows),
            adapter.get_info().backend,
            adapter.features(),
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
            match crate::d3d11_zero_copy::InteropDevice::new(adapter, _device, _queue) {
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
        self.reason.as_deref()
    }

    pub fn available(&self) -> bool {
        self.reason.is_none()
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
) -> (wgpu::Features, Option<String>) {
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
    if backend != wgpu::Backend::Vulkan {
        return (
            wgpu::Features::empty(),
            Some("zero-copy requires the Vulkan backend".into()),
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
            wgpu::Backend::Vulkan,
            INTEROP_FEATURES,
        );
        assert!(disabled.0.is_empty());
        assert!(disabled.1.unwrap().starts_with("disabled"));

        let missing = device_feature_decision(
            ZeroCopyPreference::Enabled,
            true,
            wgpu::Backend::Vulkan,
            INTEROP_FEATURES - wgpu::Features::VULKAN_WIN32_KEYED_MUTEX,
        );
        assert!(missing.0.is_empty());
        assert!(missing.1.unwrap().contains("VULKAN_WIN32_KEYED_MUTEX"));

        let ready = device_feature_decision(
            ZeroCopyPreference::Enabled,
            true,
            wgpu::Backend::Vulkan,
            INTEROP_FEATURES,
        );
        assert_eq!(ready, (INTEROP_FEATURES, None));
    }
}
