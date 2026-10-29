//! Serial console: switches, routers, and embedded boards.
//!
//! `serialport` opens the device; this module wraps it in the same shape as a
//! telnet connection so the terminal UI does not need a third code path.

use serde::Serialize;
use std::io::{Read, Write};

/// A serial port the user can pick.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SerialPortInfo {
    /// The device path, e.g. `/dev/tty.usbserial-1420`.
    pub path: String,
    /// A human label; on macOS this is usually the same as the path.
    pub name: String,
    /// USB vendor/product when the driver reports them.
    pub vendor: Option<String>,
    pub product: Option<String>,
}

/// Baud rates offered in the UI. Any rate is accepted by the backend; these are
/// the ones console cables actually use.
pub const COMMON_BAUD_RATES: &[u32] = &[
    300, 1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200, 230400,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataBits {
    Five,
    Six,
    Seven,
    Eight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parity {
    None,
    Odd,
    Even,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopBits {
    One,
    Two,
}

/// The framing a console expects. Most are 8N1, but old gear is not.
#[derive(Debug, Clone, Copy)]
pub struct SerialConfig {
    pub baud: u32,
    pub data_bits: DataBits,
    pub parity: Parity,
    pub stop_bits: StopBits,
}

impl Default for SerialConfig {
    fn default() -> Self {
        Self {
            baud: 115200,
            data_bits: DataBits::Eight,
            parity: Parity::None,
            stop_bits: StopBits::One,
        }
    }
}

/// Lists the serial ports present on this machine.
pub fn list_ports() -> Result<Vec<SerialPortInfo>, String> {
    let ports = serialport::available_ports().map_err(|e| format!("无法枚举串口：{e}"))?;

    Ok(ports
        .into_iter()
        .map(|p| match p.port_type {
            serialport::SerialPortType::UsbPort(usb) => SerialPortInfo {
                name: p.port_name.clone(),
                path: p.port_name,
                vendor: usb
                    .manufacturer
                    .clone()
                    .or_else(|| Some(format!("{:04x}", usb.vid))),
                product: usb.product,
            },
            _ => SerialPortInfo {
                name: p.port_name.clone(),
                path: p.port_name,
                vendor: None,
                product: None,
            },
        })
        .collect())
}

/// An open serial port.
pub struct SerialConnection {
    port: Box<dyn serialport::SerialPort>,
    /// Set by [`SerialConnection::start`] so the reader thread can stop.
    running: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl SerialConnection {
    pub fn open(path: &str, config: SerialConfig) -> Result<Self, String> {
        let data_bits = match config.data_bits {
            DataBits::Five => serialport::DataBits::Five,
            DataBits::Six => serialport::DataBits::Six,
            DataBits::Seven => serialport::DataBits::Seven,
            DataBits::Eight => serialport::DataBits::Eight,
        };
        let parity = match config.parity {
            Parity::None => serialport::Parity::None,
            Parity::Odd => serialport::Parity::Odd,
            Parity::Even => serialport::Parity::Even,
        };
        let stop_bits = match config.stop_bits {
            StopBits::One => serialport::StopBits::One,
            StopBits::Two => serialport::StopBits::Two,
        };

        let port = serialport::new(path, config.baud)
            .data_bits(data_bits)
            .parity(parity)
            .stop_bits(stop_bits)
            .flow_control(serialport::FlowControl::None)
            // A console can sit silent for minutes; a read timeout would turn
            // that into an error instead of "nothing yet".
            .timeout(std::time::Duration::from_millis(100))
            .open()
            .map_err(|e| format!("无法打开串口 {path}：{e}"))?;

        Ok(Self {
            port,
            running: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
        })
    }

    pub fn write(&mut self, data: &[u8]) -> Result<(), String> {
        self.port
            .write_all(data)
            .map_err(|e| format!("串口写入失败：{e}"))
    }

    /// Splits the port into a blocking writer and a reader thread.
    ///
    /// `serialport` reads block, so the reader runs on its own OS thread; the
    /// writer stays here for the async side to call.
    pub fn start(
        self,
        sink: impl Fn(Vec<u8>) + Send + 'static,
    ) -> (SerialHandle, std::sync::Arc<std::sync::atomic::AtomicBool>) {
        let running = self.running.clone();

        // The underlying port is not Sync, so the writer gets its own try_clone
        // and the original moves into the reader thread.
        let writer = match self.port.try_clone() {
            Ok(port) => port,
            Err(e) => {
                return (
                    SerialHandle {
                        inner: Err(format!("无法复制串口句柄：{e}")),
                        _handle: None,
                    },
                    running,
                )
            }
        };

        let mut reader_port = self.port;
        let reader_running = running.clone();
        let handle = std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while reader_running.load(std::sync::atomic::Ordering::SeqCst) {
                match reader_port.read(&mut buf) {
                    Ok(0) => continue,
                    Ok(n) => sink(buf[..n].to_vec()),
                    Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => continue,
                    Err(_) => break,
                }
            }
        });

        (
            SerialHandle {
                inner: Ok(std::sync::Arc::new(std::sync::Mutex::new(writer))),
                _handle: Some(handle),
            },
            running,
        )
    }
}

/// The writing half of an open serial port.
pub struct SerialHandle {
    inner: Result<std::sync::Arc<std::sync::Mutex<Box<dyn serialport::SerialPort + Send>>>, String>,
    _handle: Option<std::thread::JoinHandle<()>>,
}

impl SerialHandle {
    pub fn write(&self, data: &[u8]) -> Result<(), String> {
        let port = self.inner.as_ref().map_err(|e| e.clone())?;
        let mut port = port.lock().map_err(|e| e.to_string())?;
        port.write_all(data)
            .map_err(|e| format!("串口写入失败：{e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_usual_console_framing() {
        let c = SerialConfig::default();
        assert_eq!(c.baud, 115200);
        assert_eq!(c.data_bits, DataBits::Eight);
        assert_eq!(c.parity, Parity::None);
        assert_eq!(c.stop_bits, StopBits::One);
    }

    #[test]
    fn common_baud_rates_include_115200() {
        assert!(COMMON_BAUD_RATES.contains(&115200));
        assert!(COMMON_BAUD_RATES.contains(&9600));
    }

    // Listing ports must not panic when nothing is plugged in.
    #[test]
    fn listing_ports_does_not_panic() {
        let ports = list_ports();
        assert!(ports.is_ok() || ports.is_err());
    }
}
