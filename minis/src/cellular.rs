//! Installing without Wi-Fi: mobile data is switched off with a Shortcut for
//! the install and back on after it (`TurnOffData` / `TurnOnData`), so nothing
//! leaves the iPhone but the loopback to 10.7.0.1 that LocalDevVPN carries.
//!
//! Only steps 6-7 run with data off. Signing needs Apple's servers, so it
//! happens before. Opt in with `--data-off`.
//!
//! Each Shortcut should end with "Open App › Minis": running a Shortcut puts
//! the Shortcuts app in front, and iOS pauses a Minis left behind it.

use std::net::SocketAddr;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tokio::net::TcpStream;
use tokio::time::{sleep, timeout};

use crate::ui;

const OFF: &str = "TurnOffData";
const ON: &str = "TurnOnData";

/// Data stays off while this is alive and comes back when it is dropped,
/// on an error return too.
pub struct DataOff {
    active: bool,
}

impl DataOff {
    /// `enabled` false: does nothing, and drops to nothing.
    pub async fn start(enabled: bool) -> DataOff {
        if !enabled {
            return DataOff { active: false };
        }
        ui::detail("turning mobile data off");
        let active = run(OFF);
        if active {
            // Let the Shortcut finish and Minis come back to the front.
            sleep(Duration::from_secs(3)).await;
        } else {
            ui::detail("could not run the TurnOffData Shortcut, installing as it is");
        }
        DataOff { active }
    }
}

impl Drop for DataOff {
    fn drop(&mut self) {
        if self.active {
            if !run(ON) {
                ui::detail("could not run TurnOnData: turn mobile data back on yourself");
            }
        }
    }
}

/// Runs a Shortcut through Open Minis's `apple-open`. True if it was started.
fn run(name: &str) -> bool {
    let url = format!("shortcuts://run-shortcut?name={}", encode(name));
    Command::new("apple-open")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn encode(name: &str) -> String {
    name.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Waits, retrying, until `addr` accepts a connection: the iPhone's pairing
/// service can take a moment to answer after the network changes.
pub async fn wait_for(addr: SocketAddr, longest: Duration) -> bool {
    let until = Instant::now() + longest;
    loop {
        if let Ok(Ok(_)) = timeout(Duration::from_millis(800), TcpStream::connect(addr)).await {
            return true;
        }
        if Instant::now() >= until {
            return false;
        }
        sleep(Duration::from_millis(300)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_percent_encoded() {
        assert_eq!(encode("TurnOffData"), "TurnOffData");
        assert_eq!(encode("Turn Off"), "Turn%20Off");
    }
}
