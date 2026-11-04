//! Whether the automatic backup may use the network right now.
//!
//! The plan can require Wi-Fi ("仅在 Wi-Fi 下上传"). What that means in practice is
//! "never spend a tethered phone's data plan": a wired link is not metered, so Ethernet
//! and Thunderbolt adapters are allowed too. When the platform cannot tell us anything we
//! allow the run instead of silently stopping backups.

#[derive(Clone, Debug, PartialEq, Eq)]
struct Port {
    /// The service name reported by the platform, e.g. ``Wi-Fi`` or ``iPhone USB``.
    name: String,
    /// The interface the service is bound to, e.g. ``en0``.
    device: String,
}

const TETHERED: [&str; 4] = ["iphone", "ipad", "usb modem", "cellular"];
const WIFI: [&str; 2] = ["wi-fi", "airport"];

/// True when a run is allowed to start. ``wifi_only`` off always allows a run.
pub fn allow_upload(wifi_only: bool) -> bool {
    if !wifi_only {
        return true;
    }
    #[cfg(target_os = "macos")]
    {
        let ports = hardware_ports();
        // Fail open: a missing command must not turn backups off for good.
        if ports.is_empty() {
            return true;
        }
        let active = ports
            .into_iter()
            .filter(|port| device_is_active(&port.device))
            .collect::<Vec<_>>();
        allows(&active)
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

/// The rule itself, kept free of any system call so it can be tested.
fn allows(active: &[Port]) -> bool {
    if active.is_empty() {
        return false;
    }
    if active.iter().any(|port| is_wifi(&port.name)) {
        return true;
    }
    if active.iter().any(|port| is_tethered(&port.name)) {
        return false;
    }
    active.iter().any(|port| is_wired(&port.name))
}

fn is_wifi(name: &str) -> bool {
    contains_any(name, &WIFI)
}

fn is_tethered(name: &str) -> bool {
    contains_any(name, &TETHERED)
}

fn is_wired(name: &str) -> bool {
    contains_any(name, &["ethernet", "thunderbolt", "usb lan", "lan"])
}

fn contains_any(name: &str, needles: &[&str]) -> bool {
    let name = name.to_lowercase();
    needles.iter().any(|needle| name.contains(needle))
}

/// Parse ``networksetup -listallhardwareports`` into service/device pairs.
fn parse_ports(output: &str) -> Vec<Port> {
    let mut ports = Vec::new();
    let mut name: Option<String> = None;
    for line in output.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("Hardware Port:") {
            name = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("Device:") {
            if let Some(name) = name.take() {
                ports.push(Port {
                    name,
                    device: value.trim().to_string(),
                });
            }
        }
    }
    ports
}

#[cfg(target_os = "macos")]
fn hardware_ports() -> Vec<Port> {
    let output = std::process::Command::new("networksetup")
        .arg("-listallhardwareports")
        .output();
    match output {
        Ok(output) => parse_ports(&String::from_utf8_lossy(&output.stdout)),
        Err(_) => Vec::new(),
    }
}

#[cfg(target_os = "macos")]
fn device_is_active(device: &str) -> bool {
    if device.is_empty() {
        return false;
    }
    std::process::Command::new("ifconfig")
        .arg(device)
        .output()
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .any(|line| line.trim() == "status: active")
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(name: &str) -> Port {
        Port {
            name: name.to_string(),
            device: "en0".to_string(),
        }
    }

    #[test]
    fn wifi_only_off_never_reads_the_host() {
        assert!(allow_upload(false));
    }

    #[test]
    fn parses_service_device_pairs() {
        let ports = parse_ports(
            "Hardware Port: Ethernet Adapter (en3)\nDevice: en3\nEthernet Address: aa\n\n\
             Hardware Port: Wi-Fi\nDevice: en0\nEthernet Address: bb\n",
        );
        assert_eq!(
            ports,
            vec![
                Port {
                    name: "Ethernet Adapter (en3)".into(),
                    device: "en3".into()
                },
                Port {
                    name: "Wi-Fi".into(),
                    device: "en0".into()
                },
            ]
        );
    }

    #[test]
    fn wifi_allows_the_run() {
        assert!(allows(&[port("Wi-Fi")]));
    }

    #[test]
    fn wired_allows_the_run() {
        assert!(allows(&[port("Ethernet")]));
        assert!(allows(&[port("Thunderbolt Bridge")]));
    }

    #[test]
    fn tethering_blocks_the_run() {
        assert!(!allows(&[port("iPhone USB")]));
        assert!(!allows(&[port("iPad USB")]));
    }

    #[test]
    fn nothing_active_blocks_the_run() {
        assert!(!allows(&[]));
    }
}
