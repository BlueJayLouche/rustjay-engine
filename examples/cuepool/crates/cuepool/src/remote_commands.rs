/// If a cue command starts with `udp:` (case-insensitive), return the trimmed
/// remainder to send as a raw UDP datagram instead of an OSC packet.
pub(crate) fn strip_udp_prefix(command: &str) -> Option<&str> {
    let command = command.trim();
    if command.get(..4)?.eq_ignore_ascii_case("udp:") {
        Some(command[4..].trim())
    } else {
        None
    }
}

/// Resolve a `udp:` remainder into (host, payload).
///
/// If the remainder contains a `:`, the trimmed segment before it is a target
/// candidate: a case-insensitive match against `targets` names wins, then a
/// literal IPv4 address; payload is everything after that colon. Anything
/// else (no colon, or an unresolved candidate) sends the whole remainder to
/// `default_host` — keeping bare `udp:PLAY x.mp4` cues and colon-containing
/// filenames working.
pub(crate) fn resolve_udp_command<'a>(remainder: &'a str, targets: &[cuepool_core::UdpTarget], default_host: &str) -> (String, &'a str) {
    if let Some(idx) = remainder.find(':') {
        let candidate = remainder[..idx].trim();
        let payload = remainder[idx + 1..].trim();
        if let Some(t) = targets.iter().find(|t| t.name.eq_ignore_ascii_case(candidate)) {
            log::info!("UDP target '{}' resolved to {}", t.name, t.host);
            return (t.host.clone(), payload);
        }
        if candidate.parse::<std::net::Ipv4Addr>().is_ok() {
            log::info!("UDP target '{}' used as a literal IPv4 address", candidate);
            return (candidate.to_string(), payload);
        }
        log::warn!("UDP target '{}' not found in registry, treating whole command as payload", candidate);
    }
    (default_host.to_string(), remainder)
}

/// Send `payload` as a single raw UTF-8 UDP datagram to `host:port`.
/// Broadcast targets require `set_broadcast(true)`; failures are logged.
pub(crate) fn send_udp_command(payload: &str, host: &str, port: u16) {
    if payload.is_empty() {
        log::warn!("UDP command is empty, nothing sent");
        return;
    }
    let send = || -> std::io::Result<()> {
        let socket = std::net::UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, 0))?;
        socket.set_broadcast(true)?;
        socket.send_to(payload.as_bytes(), (host, port))?;
        Ok(())
    };
    match send() {
        Ok(_) => log::info!("UDP TX -> {}:{}: {}", host, port, payload),
        Err(e) => log::error!("UDP send to {}:{} failed: {}", host, port, e),
    }
}

/// Parse an OSC command string like `/qplayer/go,5,hello` into an `OscMessage`.
/// The first segment (before any comma) is the OSC address.
/// Remaining segments are auto-typed arguments: int → float → string.
pub(crate) fn parse_osc_command(command: &str) -> anyhow::Result<rosc::OscMessage> {
    if command.is_empty() {
        anyhow::bail!("Empty OSC command");
    }
    let parts: Vec<&str> = command.split(',').collect();
    let addr = parts[0].trim().to_string();
    if !addr.starts_with('/') {
        anyhow::bail!("OSC address must start with /: {}", addr);
    }
    let mut args = Vec::new();
    for part in &parts[1..] {
        let s = part.trim();
        if s.is_empty() {
            continue;
        }
        // Try int first
        if let Ok(i) = s.parse::<i32>() {
            args.push(rosc::OscType::Int(i));
            continue;
        }
        // Try float
        if let Ok(f) = s.parse::<f32>() {
            args.push(rosc::OscType::Float(f));
            continue;
        }
        // Default to string
        args.push(rosc::OscType::String(s.to_string()));
    }
    Ok(rosc::OscMessage { addr, args })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_osc_command_address_only() {
        let msg = parse_osc_command("/qplayer/go").unwrap();
        assert_eq!(msg.addr, "/qplayer/go");
        assert!(msg.args.is_empty());
    }

    #[test]
    fn test_parse_osc_command_with_args() {
        let msg = parse_osc_command("/qplayer/go,5,2.5,hello").unwrap();
        assert_eq!(msg.addr, "/qplayer/go");
        assert_eq!(msg.args.len(), 3);
        assert_eq!(msg.args[0], rosc::OscType::Int(5));
        assert_eq!(msg.args[1], rosc::OscType::Float(2.5));
        assert_eq!(msg.args[2], rosc::OscType::String("hello".into()));
    }

    #[test]
    fn test_parse_osc_command_invalid_address() {
        let err = parse_osc_command("cuepool/go");
        assert!(err.is_err());
    }

    #[test]
    fn test_parse_osc_command_empty() {
        let err = parse_osc_command("");
        assert!(err.is_err());
    }

    #[test]
    fn test_strip_udp_prefix() {
        assert_eq!(strip_udp_prefix("udp:PLAY myfile.mp4"), Some("PLAY myfile.mp4"));
        assert_eq!(strip_udp_prefix("  UDP:stop  "), Some("stop"));
        assert_eq!(strip_udp_prefix("udp:"), Some(""));
        assert_eq!(strip_udp_prefix("/qplayer/go,5"), None);
        assert_eq!(strip_udp_prefix("udp"), None);
        assert_eq!(strip_udp_prefix(""), None);
    }

    #[test]
    fn test_resolve_udp_command() {
        let targets = vec![
            cuepool_core::UdpTarget { name: "left".into(), host: "10.0.0.11".into() },
            cuepool_core::UdpTarget { name: "right".into(), host: "brightsign-right.local".into() },
        ];
        let default = "255.255.255.255";
        fn resolve<'a>(cmd: &'a str, targets: &[cuepool_core::UdpTarget], default: &str) -> (String, &'a str) {
            resolve_udp_command(strip_udp_prefix(cmd).unwrap(), targets, default)
        }

        // Named target hit
        assert_eq!(resolve("udp:left:PLAY a.mp4", &targets, default), ("10.0.0.11".to_string(), "PLAY a.mp4"));
        // Case-insensitive name match
        assert_eq!(resolve("udp:LEFT:stop", &targets, default), ("10.0.0.11".to_string(), "stop"));
        // Hostname target
        assert_eq!(resolve("udp:right:reboot", &targets, default), ("brightsign-right.local".to_string(), "reboot"));
        // Raw IPv4 escape hatch
        assert_eq!(resolve("udp:10.0.0.99:reboot", &targets, default), ("10.0.0.99".to_string(), "reboot"));
        // Unknown name falls back to whole payload + default host
        assert_eq!(resolve("udp:lef:PLAY a.mp4", &targets, default), (default.to_string(), "lef:PLAY a.mp4"));
        // No colon: whole remainder is the payload
        assert_eq!(resolve("udp:PLAY a.mp4", &targets, default), (default.to_string(), "PLAY a.mp4"));
        // Colon-containing filename with no target: unresolved candidate falls back
        assert_eq!(resolve("udp:PLAY C:drive.mp4", &targets, default), (default.to_string(), "PLAY C:drive.mp4"));
        // Empty payload after a resolved target (send_udp_command warns and skips)
        assert_eq!(resolve("udp:left:", &targets, default), ("10.0.0.11".to_string(), ""));
    }
}
