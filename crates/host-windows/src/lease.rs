//! Pairing is still available without a certificate. A playback heartbeat must prove possession of
//! a currently enabled, exactly pinned Moonlight identity through the TLS client handshake.
use rustls::client::danger::HandshakeSignatureValid;
use rustls::pki_types::{pem::PemObject, CertificateDer, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{CertificateError, DigitallySignedStruct, DistinguishedName, Error, SignatureScheme};
use sha2::{Digest, Sha256};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub struct PairedClients(pub PathBuf);
impl PairedClients {
    pub fn enabled(&self) -> io::Result<Vec<CertificateDer<'static>>> {
        let data: serde_json::Value = serde_json::from_slice(&fs::read(&self.0)?)?;
        let mut certs = Vec::new();
        if let Some(devices) = data
            .pointer("/root/named_devices")
            .and_then(|v| v.as_array())
        {
            for d in devices {
                if let Some(enabled) = d.get("enabled") {
                    if enabled != true && enabled != "true" {
                        continue;
                    }
                }
                let pem = d["cert"]
                    .as_str()
                    .ok_or_else(|| io::Error::other("缺少设备证书"))?;
                certs.push(
                    CertificateDer::from_pem_slice(pem.as_bytes()).map_err(io::Error::other)?,
                );
            }
        }
        Ok(certs)
    }
}
pub fn sole_owner(dir: &Path) -> Result<[u8; 32], String> {
    let certs = match PairedClients(dir.join("flashlink-state.json")).enabled() {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
        result => result.map_err(|e| e.to_string())?,
    };
    match certs.as_slice() {
        [cert] => Ok(Sha256::digest(cert.as_ref()).into()),
        [] => Err("请先在安卓设备上连接这台电脑并完成配对，再开启防窥模式。".into()),
        _ => Err("开启防窥模式需要恰好一台已授权设备。请在设备管理中移除其余设备。".into()),
    }
}
impl ClientCertVerifier for PairedClients {
    fn client_auth_mandatory(&self) -> bool {
        false
    }
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }
    fn verify_client_cert(
        &self,
        cert: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: UnixTime,
    ) -> Result<ClientCertVerified, Error> {
        if self
            .enabled()
            .is_ok_and(|list| list.iter().any(|known| known == cert))
        {
            Ok(ClientCertVerified::assertion())
        } else {
            Err(Error::InvalidCertificate(CertificateError::UnknownIssuer))
        }
    }
    fn verify_tls12_signature(
        &self,
        msg: &[u8],
        cert: &CertificateDer<'_>,
        sig: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls12_signature(
            msg,
            cert,
            sig,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        msg: &[u8],
        cert: &CertificateDer<'_>,
        sig: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls13_signature(
            msg,
            cert,
            sig,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn privacy_requires_exactly_one_enabled_pin_and_honors_revocation() {
        let dir =
            std::env::temp_dir().join(format!("flashlink-client-pins-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("flashlink-state.json");
        let a = "-----BEGIN CERTIFICATE-----\nAQID\n-----END CERTIFICATE-----\n";
        let b = "-----BEGIN CERTIFICATE-----\nBAUG\n-----END CERTIFICATE-----\n";
        let write = |devices| {
            fs::write(&file, json!({"root":{"named_devices":devices}}).to_string()).unwrap()
        };
        let leaf = CertificateDer::from_pem_slice(a.as_bytes()).unwrap();
        let verifier = PairedClients(file.clone());
        assert!(sole_owner(&dir).unwrap_err().contains("完成配对"));
        write(json!([]));
        assert!(sole_owner(&dir).unwrap_err().contains("完成配对"));
        write(json!([{"cert":a,"enabled":"true"},{"cert":b,"enabled":"false"}]));
        assert_eq!(
            sole_owner(&dir).unwrap(),
            <[u8; 32]>::from(Sha256::digest(leaf.as_ref()))
        );
        assert!(verifier
            .verify_client_cert(&leaf, &[], UnixTime::now())
            .is_ok());
        write(json!([{"cert":a},{"cert":b,"enabled":true}]));
        assert!(sole_owner(&dir).unwrap_err().contains("其余设备"));
        write(json!([{"cert":a,"enabled":false},{"cert":b,"enabled":true}]));
        assert!(verifier
            .verify_client_cert(&leaf, &[], UnixTime::now())
            .is_err());
        fs::remove_dir_all(dir).unwrap();
    }
}
