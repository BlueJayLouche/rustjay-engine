//! Destination choices for the OSC settings dropdown.
//!
//! Enumeration feeds the picker and nothing else. No socket is bound to a
//! chosen interface and no egress interface is selected — picking a row writes
//! an address into `ShowSettings::osc_tx_host`, and the OSC driver sends there.
//! That indirection is deliberate: the setting this replaced named a network
//! card but only ever derived a destination from it, which is why a blank field
//! silently sent everything to loopback (#213).

use std::net::Ipv4Addr;

/// A row in the destination dropdown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DestinationChoice {
    /// Written verbatim into `osc_tx_host` when the row is picked.
    pub address: String,
    /// What the row reads in the dropdown.
    pub label: String,
}

/// A local interface, reduced to what choosing a destination needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nic {
    pub name: String,
    pub ip: Ipv4Addr,
    pub netmask: Ipv4Addr,
}

impl Nic {
    /// The directed broadcast for this card's subnet — every host on the wire
    /// this card is plugged into, and nothing beyond the first router.
    fn broadcast(&self) -> Ipv4Addr {
        let ip = u32::from_be_bytes(self.ip.octets());
        let mask = u32::from_be_bytes(self.netmask.octets());
        Ipv4Addr::from(ip | !mask)
    }

    fn prefix_len(&self) -> u32 {
        u32::from_be_bytes(self.netmask.octets()).count_ones()
    }
}

/// The two fixed destinations, then one directed broadcast per interface.
///
/// Loopback leads because it is the default: a machine that has not been told
/// where to send should not be putting packets onto whatever network it happens
/// to be plugged into.
pub fn destination_choices(nics: &[Nic]) -> Vec<DestinationChoice> {
    let mut choices = vec![
        DestinationChoice {
            address: Ipv4Addr::LOCALHOST.to_string(),
            label: "Loopback — this machine only".into(),
        },
        DestinationChoice {
            address: Ipv4Addr::BROADCAST.to_string(),
            label: "Broadcast — all networks".into(),
        },
    ];

    for nic in nics {
        // Loopback already has its own entry above, and its directed broadcast
        // (127.255.255.255) is a worse spelling of the same thing.
        if nic.ip.is_loopback() {
            continue;
        }
        // A /31 or /32 is a point-to-point link — VPN tunnels show up this way.
        // Masking leaves the card's own address, so the row would offer a
        // "broadcast" that is really a unicast back to this machine.
        if nic.prefix_len() >= 31 {
            continue;
        }
        let broadcast = nic.broadcast();
        choices.push(DestinationChoice {
            // `>` rather than an arrow: egui's bundled fonts have no U+2192 and
            // render it as tofu. The inspector's cue-command help uses `>` for
            // the same "goes to" sense.
            address: broadcast.to_string(),
            label: format!(
                "{} — {}/{} > {}",
                nic.name,
                nic.ip,
                nic.prefix_len(),
                broadcast
            ),
        });
    }

    choices
}

/// Local IPv4 interfaces. An empty list on failure: the dropdown still offers
/// loopback and broadcast, and the address can always be typed.
pub fn local_nics() -> Vec<Nic> {
    let interfaces = match if_addrs::get_if_addrs() {
        Ok(interfaces) => interfaces,
        Err(error) => {
            log::warn!(
                "Could not enumerate network interfaces for the OSC destination list: {error}"
            );
            return Vec::new();
        }
    };

    interfaces
        .into_iter()
        .filter_map(|interface| match interface.addr {
            if_addrs::IfAddr::V4(v4) => Some(Nic {
                name: interface.name,
                ip: v4.ip,
                netmask: v4.netmask,
            }),
            // OSC here is IPv4 throughout — the driver binds and sends v4.
            if_addrs::IfAddr::V6(_) => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nic(name: &str, ip: [u8; 4], netmask: [u8; 4]) -> Nic {
        Nic {
            name: name.into(),
            ip: Ipv4Addr::from(ip),
            netmask: Ipv4Addr::from(netmask),
        }
    }

    #[test]
    fn loopback_and_broadcast_are_offered_even_with_no_interfaces() {
        let choices = destination_choices(&[]);

        let addresses: Vec<&str> = choices.iter().map(|c| c.address.as_str()).collect();
        assert_eq!(addresses, ["127.0.0.1", "255.255.255.255"]);
    }

    #[test]
    fn an_interface_contributes_its_directed_broadcast() {
        let choices = destination_choices(&[nic("en0", [10, 10, 30, 196], [255, 255, 255, 0])]);

        let entry = choices.last().expect("the interface must add a row");
        assert_eq!(entry.address, "10.10.30.255");
        assert_eq!(entry.label, "en0 — 10.10.30.196/24 > 10.10.30.255");
    }

    /// The setting this replaces masked every address against a hardcoded /24,
    /// so a site on any other prefix got a broadcast address for a subnet it
    /// was not on. The picker must read the real netmask.
    #[test]
    fn a_non_slash_24_interface_uses_its_own_netmask() {
        let choices = destination_choices(&[nic("eth1", [172, 16, 4, 9], [255, 255, 0, 0])]);

        let entry = choices.last().expect("the interface must add a row");
        assert_eq!(entry.address, "172.16.255.255");
        assert_eq!(entry.label, "eth1 — 172.16.4.9/16 > 172.16.255.255");
    }

    /// A VPN tunnel is a /32: masking leaves the card's own address, so the row
    /// would offer a "broadcast" that is a unicast back to this machine. Found
    /// by rendering the real dropdown, where a `utun4` row sat among the usable
    /// ones looking exactly as legitimate.
    #[test]
    fn point_to_point_interfaces_are_not_offered() {
        let choices = destination_choices(&[
            nic("utun4", [100, 97, 190, 37], [255, 255, 255, 255]),
            nic("en0", [192, 168, 1, 50], [255, 255, 255, 0]),
        ]);

        let addresses: Vec<&str> = choices.iter().map(|c| c.address.as_str()).collect();
        assert_eq!(addresses, ["127.0.0.1", "255.255.255.255", "192.168.1.255"]);
    }

    #[test]
    fn loopback_interfaces_do_not_duplicate_the_fixed_entry() {
        let choices = destination_choices(&[
            nic("lo0", [127, 0, 0, 1], [255, 0, 0, 0]),
            nic("en0", [192, 168, 1, 50], [255, 255, 255, 0]),
        ]);

        let addresses: Vec<&str> = choices.iter().map(|c| c.address.as_str()).collect();
        assert_eq!(addresses, ["127.0.0.1", "255.255.255.255", "192.168.1.255"]);
    }
}
