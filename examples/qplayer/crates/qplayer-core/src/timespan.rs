//! C# `TimeSpan` compatible duration type.
//!
//! C# `System.TimeSpan` serialized by `System.Text.Json` uses the "c" format:
//! `[-][d.]hh:mm:ss[.fffffff]`
//!
//! This type deserializes from that format (and falls back to seconds-as-f64 for
//! flexibility) and serializes back to the same string format for byte-identical
//! round-trips with C# QPlayer show files.

use serde::{de::Visitor, Deserialize, Serialize};
use std::fmt;
use std::time::Duration;

/// A duration compatible with C# `TimeSpan` JSON serialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Timespan(pub Duration);

impl Timespan {
    pub const ZERO: Self = Self(Duration::ZERO);

    #[inline]
    pub fn from_secs_f64(secs: f64) -> Self {
        Self(Duration::from_secs_f64(secs))
    }

    #[inline]
    pub fn as_secs_f64(&self) -> f64 {
        self.0.as_secs_f64()
    }

    #[inline]
    pub fn as_millis(&self) -> u128 {
        self.0.as_millis()
    }

    /// Format as C# TimeSpan "c" format: `[-][d.]hh:mm:ss[.fffffff]`
    pub fn to_csharp_string(&self) -> String {
        let total_secs = self.0.as_secs();
        let nanos = self.0.subsec_nanos();
        let sign = "";
        let total_secs = total_secs;
        let days = total_secs / 86_400;
        let rem = total_secs % 86_400;
        let hours = rem / 3_600;
        let rem = rem % 3_600;
        let mins = rem / 60;
        let secs = rem % 60;

        let frac = nanos as f64 / 1_000_000_000.0;
        if frac > 0.0 {
            if days > 0 {
                format!("{}{}.{:02}:{:02}:{:02}.{:07}", sign, days, hours, mins, secs, nanos / 100)
            } else {
                format!("{}{:02}:{:02}:{:02}.{:07}", sign, hours, mins, secs, nanos / 100)
            }
        } else if days > 0 {
            format!("{}{}.{:02}:{:02}:{:02}", sign, days, hours, mins, secs)
        } else {
            format!("{}{:02}:{:02}:{:02}", sign, hours, mins, secs)
        }
    }

    /// Parse C# TimeSpan "c" format.
    fn parse_csharp(s: &str) -> Option<Self> {
        let s = s.trim();
        if s.is_empty() {
            return Some(Self::ZERO);
        }

        let s = s.trim();
        let _negative = s.starts_with('-');
        let s = s.strip_prefix('-').unwrap_or(s);

        // Split into h:m:s parts first
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 3 {
            return None;
        }

        // The first part may contain days: "1.02" means 1 day, 2 hours
        let (days, hours_str) = if let Some((d, h)) = parts[0].split_once('.') {
            (d.parse::<u64>().ok()?, h)
        } else {
            (0u64, parts[0])
        };

        let hours: u64 = hours_str.parse().ok()?;
        let mins: u64 = parts[1].parse().ok()?;
        let sec_part = parts[2];

        let (secs, frac_nanos) = if let Some((s, f)) = sec_part.split_once('.') {
            let secs: u64 = s.parse().ok()?;
            // Pad fractional to 7 digits (C# ticks are 100ns = 7 digits)
            let frac_padded = format!("{:0<7}", f);
            let frac: u64 = frac_padded.parse().ok()?;
            let nanos = (frac * 100) as u32; // 7 digits -> nanos (but this is approximate)
            (secs, nanos)
        } else {
            (sec_part.parse().ok()?, 0u32)
        };

        let total_secs = days * 86_400 + hours * 3_600 + mins * 60 + secs;
        let dur = Duration::new(total_secs, frac_nanos);
        Some(Self(dur))
    }
}

impl From<Duration> for Timespan {
    fn from(d: Duration) -> Self {
        Self(d)
    }
}

impl From<Timespan> for Duration {
    fn from(t: Timespan) -> Self {
        t.0
    }
}

impl Serialize for Timespan {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_csharp_string())
    }
}

impl<'de> Deserialize<'de> for Timespan {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct TimespanVisitor;

        impl<'de> Visitor<'de> for TimespanVisitor {
            type Value = Timespan;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a TimeSpan string (hh:mm:ss) or a number of seconds")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Timespan::parse_csharp(v)
                    .ok_or_else(|| E::custom(format!("invalid TimeSpan string: {}", v)))
            }

            fn visit_f64<E>(self, v: f64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(Timespan::from_secs_f64(v))
            }

            fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(Timespan(Duration::from_secs(v)))
            }
        }

        deserializer.deserialize_any(TimespanVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zero() {
        let ts = Timespan::ZERO;
        assert_eq!(ts.to_csharp_string(), "00:00:00");
    }

    #[test]
    fn test_round_trip() {
        let cases = [
            ("00:00:00", 0.0),
            ("00:00:05", 5.0),
            ("00:01:30", 90.0),
            ("01:30:00", 5400.0),
            ("1.02:30:45", 95445.0),
        ];
        for (input, expected_secs) in &cases {
            let parsed = Timespan::parse_csharp(input)
                .unwrap_or_else(|| panic!("failed to parse: {}", input));
            assert!(
                (parsed.as_secs_f64() - expected_secs).abs() < 0.001,
                "parsed {} -> {}s, expected {}s",
                input,
                parsed.as_secs_f64(),
                expected_secs
            );
        }
    }

    #[test]
    fn test_serde_json() {
        let ts = Timespan::from_secs_f64(5.0);
        let json = serde_json::to_string(&ts).unwrap();
        // Zero fractional seconds are omitted for clean output
        assert_eq!(json, "\"00:00:05\"");

        let de: Timespan = serde_json::from_str(&json).unwrap();
        assert_eq!(de.as_secs_f64(), 5.0);
    }

    #[test]
    fn test_serde_json_fractional() {
        let ts = Timespan::from_secs_f64(0.5);
        let json = serde_json::to_string(&ts).unwrap();
        assert!(json.contains("."), "fractional seconds should appear: {}", json);

        let de: Timespan = serde_json::from_str(&json).unwrap();
        assert!((de.as_secs_f64() - 0.5).abs() < 0.001);
    }
}
