//! Who may connect, and how a phone is let in the first time.
//!
//! Each side has a key of its own, kept in a file. The desktop shows a
//! pairing code, as a picture to scan: its address and a secret that
//! works once and not for long. A phone that presents the secret is
//! remembered by its key and needs no code after that.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use iroh::{EndpointAddr, SecretKey};
use serde::{Deserialize, Serialize};

/// Begins every pairing code, so that something else scanned by mistake
/// is told apart.
const PREFIX: &str = "cortex-pair:";

/// What the desktop shows for a phone to scan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ticket {
    /// Where the desktop is reached.
    pub addr: EndpointAddr,

    /// Lets one phone in, once.
    pub token: String,

    /// The desktop's name, for the phone's list.
    pub name: String,
}

impl Ticket {
    pub fn encode(&self) -> String {
        let text = serde_json::to_vec(self).expect("a ticket is plain data");

        format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(text))
    }

    pub fn decode(code: &str) -> Result<Self> {
        let Some(rest) = code.trim().strip_prefix(PREFIX) else {
            bail!("This is not a Cortex pairing code.");
        };

        let text = URL_SAFE_NO_PAD
            .decode(rest)
            .context("The pairing code is damaged.")?;

        serde_json::from_slice(&text).context("The pairing code is damaged.")
    }
}

/// A phone the desktop lets in, or a desktop a phone knows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Device {
    /// Its public key.
    pub id: String,

    pub name: String,

    /// Milliseconds since 1970.
    pub paired_ms: u64,

    /// Where it was last reached. Kept by a phone for its desktops.
    #[serde(default)]
    pub addr: Option<EndpointAddr>,
}

/// One side's key and the other sides it knows, in a file.
#[derive(Debug)]
pub struct Keyring {
    file: PathBuf,
    kept: Kept,
}

#[derive(Debug, Serialize, Deserialize)]
struct Kept {
    /// This side's secret key, in hex.
    secret: String,

    #[serde(default)]
    devices: Vec<Device>,
}

impl Keyring {
    /// Reads the file, or starts one with a new key.
    pub fn open(file: &Path) -> Result<Self> {
        let kept = match fs::read_to_string(file) {
            Ok(text) => serde_json::from_str(&text)
                .with_context(|| format!("cannot read {}", file.display()))?,

            Err(_) => Kept {
                secret: hex(&SecretKey::generate().to_bytes()),
                devices: Vec::new(),
            },
        };

        let keyring = Self {
            file: file.to_path_buf(),
            kept,
        };

        keyring.secret()?;
        keyring.save()?;

        Ok(keyring)
    }

    pub fn secret(&self) -> Result<SecretKey> {
        let bytes = unhex(&self.kept.secret)
            .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
            .with_context(|| format!("the key in {} is damaged", self.file.display()))?;

        Ok(SecretKey::from_bytes(&bytes))
    }

    pub fn devices(&self) -> &[Device] {
        &self.kept.devices
    }

    pub fn knows(&self, id: &str) -> bool {
        self.kept.devices.iter().any(|device| device.id == id)
    }

    pub fn find(&self, id: &str) -> Option<&Device> {
        self.kept.devices.iter().find(|device| device.id == id)
    }

    /// Adds the device, or replaces what was known about it.
    pub fn remember(&mut self, id: &str, name: &str, addr: Option<EndpointAddr>) -> Result<()> {
        self.kept.devices.retain(|device| device.id != id);

        self.kept.devices.push(Device {
            id: id.to_string(),
            name: name.to_string(),
            paired_ms: now_ms(),
            addr,
        });

        self.save()
    }

    pub fn forget(&mut self, id: &str) -> Result<()> {
        self.kept.devices.retain(|device| device.id != id);

        self.save()
    }

    fn save(&self) -> Result<()> {
        if let Some(folder) = self.file.parent() {
            fs::create_dir_all(folder)?;
        }

        fs::write(&self.file, serde_json::to_string_pretty(&self.kept)?)
            .with_context(|| format!("cannot write {}", self.file.display()))?;

        // The key lets its holder act as this side.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            fs::set_permissions(&self.file, fs::Permissions::from_mode(0o600))?;
        }

        Ok(())
    }
}

/// A secret that lets one phone in.
pub fn new_token() -> String {
    hex(&rand::random::<[u8; 16]>())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or(0)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }

    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(text.get(at..at + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ticket_survives_being_shown_and_scanned() {
        let ticket = Ticket {
            addr: EndpointAddr::new(SecretKey::generate().public()),
            token: new_token(),
            name: "Studio".into(),
        };

        let read = Ticket::decode(&format!("  {}\n", ticket.encode())).unwrap();

        assert_eq!(read.addr, ticket.addr);
        assert_eq!(read.token, ticket.token);
        assert_eq!(read.name, "Studio");
    }

    #[test]
    fn something_else_is_not_taken_for_a_ticket() {
        assert!(Ticket::decode("https://example.com").is_err());
        assert!(Ticket::decode("cortex-pair:!!!").is_err());
    }

    #[test]
    fn the_key_and_the_devices_are_still_there_next_time() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("remote.json");

        let mut keyring = Keyring::open(&file).unwrap();
        let key = keyring.secret().unwrap().public();

        keyring.remember("abc", "Phone", None).unwrap();

        let again = Keyring::open(&file).unwrap();

        assert_eq!(again.secret().unwrap().public(), key);
        assert!(again.knows("abc"));

        keyring.forget("abc").unwrap();

        assert!(!Keyring::open(&file).unwrap().knows("abc"));
    }
}
