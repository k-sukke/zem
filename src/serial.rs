//! USB serial ports, the way ESP boards show up on every OS (CP210x, CH34x, native USB-JTAG).

use serialport::{SerialPortType, UsbPortInfo};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbPort {
    pub name: String,
    pub vid: u16,
    pub pid: u16,
    pub product: Option<String>,
}

impl UsbPort {
    pub fn describe(&self) -> String {
        let chip = known_bridge(self.vid, self.pid)
            .map(str::to_string)
            .or_else(|| self.product.clone())
            .unwrap_or_else(|| "USB serial".to_string());
        format!("{} ({chip}, {:04x}:{:04x})", self.name, self.vid, self.pid)
    }
}

/// USB-to-UART bridges and on-chip USB found on ESP development boards.
fn known_bridge(vid: u16, pid: u16) -> Option<&'static str> {
    match (vid, pid) {
        (0x10c4, 0xea60) => Some("CP210x"),
        (0x1a86, 0x7523) | (0x1a86, 0x55d4) => Some("CH34x"),
        (0x0403, _) => Some("FTDI"),
        (0x303a, 0x1001) => Some("ESP USB-Serial/JTAG"),
        (0x303a, _) => Some("Espressif USB"),
        _ => None,
    }
}

/// USB serial ports, sorted by name. On macOS each device appears as both `/dev/tty.*` and
/// `/dev/cu.*`; only the callout (`cu`) device is kept because opening `tty` blocks.
pub fn usb_ports() -> Vec<UsbPort> {
    let ports = serialport::available_ports().unwrap_or_default();
    let mut usb: Vec<UsbPort> = ports
        .into_iter()
        .filter_map(|p| match p.port_type {
            SerialPortType::UsbPort(UsbPortInfo {
                vid, pid, product, ..
            }) => Some(UsbPort {
                name: p.port_name,
                vid,
                pid,
                product,
            }),
            _ => None,
        })
        .collect();
    dedupe_callout(&mut usb);
    usb
}

fn dedupe_callout(ports: &mut Vec<UsbPort>) {
    let names: Vec<String> = ports.iter().map(|p| p.name.clone()).collect();
    ports.retain(|p| {
        p.name
            .strip_prefix("/dev/tty.")
            .is_none_or(|rest| !names.contains(&format!("/dev/cu.{rest}")))
    });
    ports.sort_by(|a, b| a.name.cmp(&b.name));
    ports.dedup_by(|a, b| a.name == b.name);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(name: &str) -> UsbPort {
        UsbPort {
            name: name.into(),
            vid: 0x10c4,
            pid: 0xea60,
            product: None,
        }
    }

    #[test]
    fn keeps_callout_devices_only() {
        let mut ports = vec![
            port("/dev/tty.usbserial-10"),
            port("/dev/cu.usbserial-10"),
            port("/dev/ttyUSB0"),
            port("COM3"),
        ];
        dedupe_callout(&mut ports);
        let names: Vec<_> = ports.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["/dev/cu.usbserial-10", "/dev/ttyUSB0", "COM3"]);
    }

    #[test]
    fn describes_known_bridges() {
        assert_eq!(
            port("/dev/ttyUSB0").describe(),
            "/dev/ttyUSB0 (CP210x, 10c4:ea60)"
        );
    }
}
