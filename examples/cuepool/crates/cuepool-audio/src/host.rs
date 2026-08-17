//! Driver-aware cpal host selection, shared by the programme engine and the
//! LTC input/output streams. ASIO resolves only on Windows builds compiled
//! with the `asio` feature; every other driver maps to the platform default
//! host.

use cuepool_core::AudioOutputDriver;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostChoice {
    Default,
    Asio,
}

pub(crate) fn host_choice(driver: AudioOutputDriver) -> HostChoice {
    match driver {
        AudioOutputDriver::ASIO => HostChoice::Asio,
        AudioOutputDriver::WASAPI | AudioOutputDriver::Wave | AudioOutputDriver::DirectSound => {
            HostChoice::Default
        }
    }
}

/// Driver-neutral host selection failure. `engine.rs` maps this onto its
/// output-flavored `AudioError` variants; the LTC streams report it as-is.
// allow: which variants get constructed depends on the platform/`asio`
// feature cfg matrix (e.g. `Unavailable` only exists on Windows ASIO builds).
#[allow(dead_code)]
#[derive(Debug, thiserror::Error)]
pub(crate) enum HostError {
    #[error(
        "audio driver {driver} was requested, but CuePool was built without ASIO support; rebuild cuepool with `--features asio`"
    )]
    NotCompiled { driver: &'static str },
    #[error("audio driver {driver} is only supported on Windows, not {platform}")]
    UnsupportedPlatform {
        driver: &'static str,
        platform: &'static str,
    },
    #[error("audio driver {driver} is unavailable: {source}")]
    Unavailable {
        driver: &'static str,
        #[source]
        source: cpal::Error,
    },
}

pub(crate) fn host_for_driver(driver: AudioOutputDriver) -> Result<cpal::Host, HostError> {
    match host_choice(driver) {
        HostChoice::Default => Ok(cpal::default_host()),
        HostChoice::Asio => asio_host(),
    }
}

#[cfg(all(target_os = "windows", feature = "asio"))]
fn asio_host() -> Result<cpal::Host, HostError> {
    cpal::host_from_id(cpal::HostId::Asio).map_err(|source| HostError::Unavailable {
        driver: AudioOutputDriver::ASIO.name(),
        source,
    })
}

#[cfg(not(feature = "asio"))]
fn asio_host() -> Result<cpal::Host, HostError> {
    Err(HostError::NotCompiled {
        driver: AudioOutputDriver::ASIO.name(),
    })
}

#[cfg(all(feature = "asio", not(target_os = "windows")))]
fn asio_host() -> Result<cpal::Host, HostError> {
    Err(HostError::UnsupportedPlatform {
        driver: AudioOutputDriver::ASIO.name(),
        platform: std::env::consts::OS,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn driver_selection_decision_table() {
        assert_eq!(host_choice(AudioOutputDriver::WASAPI), HostChoice::Default);
        assert_eq!(host_choice(AudioOutputDriver::Wave), HostChoice::Default);
        assert_eq!(
            host_choice(AudioOutputDriver::DirectSound),
            HostChoice::Default
        );
        assert_eq!(host_choice(AudioOutputDriver::ASIO), HostChoice::Asio);
    }

    #[test]
    #[cfg(not(feature = "asio"))]
    fn asio_request_explains_missing_feature() {
        let message = asio_host().err().unwrap().to_string();
        assert!(message.contains("ASIO"));
        assert!(message.contains("--features asio"));
    }

    #[test]
    #[cfg(all(feature = "asio", not(target_os = "windows")))]
    fn asio_feature_is_a_documented_no_op_off_windows() {
        let message = asio_host().err().unwrap().to_string();
        assert!(message.contains("only supported on Windows"));
    }
}
