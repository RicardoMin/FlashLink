// SPDX-License-Identifier: GPL-3.0-only
use base64::{engine::general_purpose::STANDARD, Engine};
use rustls_pki_types::SubjectPublicKeyInfoDer;
use serde::Deserialize;
use webpki::{ring::RSA_PKCS1_2048_8192_SHA256, RawPublicKeyEntity};

include!(concat!(env!("OUT_DIR"), "/version.rs"));

pub const FEED_URL: &str =
    "https://github.com/RicardoMin/FlashLink/releases/latest/download/flashlink-update.json";
pub const MAX_FEED_BYTES: usize = 128 * 1024;
const PUBLIC_KEY: &[u8] = include_bytes!("../../../release/update-key.der");

#[derive(Clone, Debug, Deserialize)]
pub struct Asset {
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug)]
pub struct WindowsRelease {
    pub version: String,
    pub build: u32,
    pub notes: String,
    pub exe: Asset,
    pub zip: Asset,
}

#[derive(Deserialize)]
struct Envelope {
    payload: String,
    signature: String,
}

#[derive(Deserialize)]
struct Feed {
    schema: u32,
    windows: Entry,
}

#[derive(Deserialize)]
struct Entry {
    version: String,
    build: u16,
    notes: String,
    assets: WindowsAssets,
}

#[derive(Deserialize)]
struct WindowsAssets {
    exe: Asset,
    zip: Asset,
}

fn numeric_version(value: &str) -> Option<[u16; 3]> {
    let mut parts = value.split('.');
    let mut result = [0; 3];
    for part in &mut result {
        let text = parts.next()?;
        *part = text.parse().ok()?;
        if part.to_string() != text {
            return None;
        }
    }
    parts.next().is_none().then_some(result)
}

impl Asset {
    fn valid(&self, version: &str, filename: &str) -> bool {
        self.url
            == format!(
                "https://github.com/RicardoMin/FlashLink/releases/download/v{version}/{filename}"
            )
            && (1..=1024 * 1024 * 1024).contains(&self.size)
            && self.sha256.len() == 64
            && self
                .sha256
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    }
}

pub fn windows_release(bytes: &[u8]) -> Result<WindowsRelease, String> {
    verified_release(bytes, PUBLIC_KEY)
}

fn verified_release(bytes: &[u8], key: &[u8]) -> Result<WindowsRelease, String> {
    if bytes.len() > MAX_FEED_BYTES {
        return Err("更新清单过大".into());
    }
    let envelope: Envelope = serde_json::from_slice(bytes).map_err(|_| "更新清单格式无效")?;
    let payload = STANDARD
        .decode(envelope.payload)
        .map_err(|_| "更新清单编码无效")?;
    let signature = STANDARD
        .decode(envelope.signature)
        .map_err(|_| "更新签名编码无效")?;
    let spki = SubjectPublicKeyInfoDer::from(key);
    RawPublicKeyEntity::try_from(&spki)
        .and_then(|key| key.verify_signature(RSA_PKCS1_2048_8192_SHA256, &payload, &signature))
        .map_err(|_| "无法验证 FlashLink 更新清单签名")?;
    let feed: Feed = serde_json::from_slice(&payload).map_err(|_| "更新清单内容无效")?;
    let entry = feed.windows;
    let version = numeric_version(&entry.version).ok_or("Windows 版本号无效")?;
    if feed.schema != 1
        || entry.build == 0
        || entry.notes.trim().is_empty()
        || entry.notes.len() > 4096
        || !entry
            .assets
            .exe
            .valid(&entry.version, "FlashLink-Windows-x64.exe")
        || !entry
            .assets
            .zip
            .valid(&entry.version, "FlashLink-Windows-x64.zip")
    {
        return Err("Windows 更新信息无效".into());
    }
    if u32::from(entry.build) > WINDOWS_BUILD && version < numeric_version(WINDOWS_VERSION).unwrap()
    {
        return Err("Windows 更新的版本和构建号不一致".into());
    }
    Ok(WindowsRelease {
        version: entry.version,
        build: entry.build.into(),
        notes: entry.notes,
        exe: entry.assets.exe,
        zip: entry.assets.zip,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_an_unsigned_or_oversized_feed() {
        assert!(windows_release(br#"{"payload":"e30=","signature":""}"#).is_err());
        assert!(windows_release(&vec![0; MAX_FEED_BYTES + 1]).is_err());
    }

    #[test]
    fn accepts_a_signed_feed_and_rejects_a_changed_payload() {
        let fixture = include_bytes!("../tests/signed-feed.json");
        let key = include_bytes!("../tests/fixture-key.der");
        let entry = verified_release(fixture, key).unwrap();
        assert_eq!((entry.version.as_str(), entry.build), ("0.4.0", 320));
        assert!(windows_release(fixture).is_err());
        let mut envelope: serde_json::Value = serde_json::from_slice(fixture).unwrap();
        let mut data = STANDARD
            .decode(envelope["payload"].as_str().unwrap())
            .unwrap();
        data.push(b' ');
        envelope["payload"] = STANDARD.encode(data).into();
        assert!(verified_release(&serde_json::to_vec(&envelope).unwrap(), key).is_err());
    }

    #[test]
    fn requires_the_official_versioned_asset() {
        let mut entry = verified_release(
            include_bytes!("../tests/signed-feed.json"),
            include_bytes!("../tests/fixture-key.der"),
        )
        .unwrap();
        entry.exe.url = entry.zip.url;
        assert!(!entry.exe.valid(&entry.version, "FlashLink-Windows-x64.exe"));
        assert!(numeric_version("1.2.03").is_none());
        assert!(numeric_version("1.2.0/other").is_none());
    }
}
