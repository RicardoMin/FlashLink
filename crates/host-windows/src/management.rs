use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::json;
#[cfg(windows)]
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

pub struct Credentials {
    pub authorization: String,
}

impl Credentials {
    pub fn create(path: &Path) -> Result<Self, String> {
        let mut random = [0u8; 48];
        getrandom::fill(&mut random).map_err(|e| format!("生成本机管理密钥失败：{e}"))?;
        let password = STANDARD.encode(&random[..32]);
        let salt = STANDARD.encode(&random[32..]);
        let username = "flashlink";
        let contents = json!({
            "username": username,
            "salt": salt,
            "password": sunshine_hash(&password, &salt),
        });
        fs::write(path, contents.to_string()).map_err(|e| format!("无法写入管理身份文件：{e}"))?;
        Ok(Self {
            authorization: format!(
                "Basic {}",
                STANDARD.encode(format!("{username}:{password}"))
            ),
        })
    }
}

fn sunshine_hash(password: &str, salt: &str) -> String {
    // Upstream uses util::hex(crypto::hash(password + salt)), whose default is reverse byte order.
    let digest = Sha256::digest([password.as_bytes(), salt.as_bytes()].concat());
    digest.iter().rev().map(|b| format!("{b:02X}")).collect()
}

#[cfg(windows)]
#[derive(serde::Deserialize)]
pub struct Pending {
    pub id: String,
    pub address: std::net::IpAddr,
    pub name: String,
}

#[cfg(windows)]
pub struct Api {
    port: u16,
    authorization: String,
    cert_der: Vec<u8>,
    tls: native_tls::TlsConnector,
}

#[cfg(windows)]
impl Api {
    pub fn new(base: u16, cert_path: &Path, creds: Credentials) -> Result<Self, String> {
        let pem = fs::read(cert_path).map_err(|e| format!("读取本机证书失败：{e}"))?;
        let cert = native_tls::Certificate::from_pem(&pem).map_err(|e| e.to_string())?;
        // Sunshine's self-signed GameStream certificate does not name localhost. The exact
        // leaf DER is verified on each established channel before any HTTP data leaves it.
        let tls = native_tls::TlsConnector::builder()
            .danger_accept_invalid_certs(true)
            .danger_accept_invalid_hostnames(true)
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            port: base + 1,
            authorization: creds.authorization,
            cert_der: cert.to_der().map_err(|e| e.to_string())?,
            tls,
        })
    }

    fn request(&self, method: &str, body: Value) -> Result<Value, String> {
        use std::io::{BufRead, BufReader, Read, Write};
        use std::net::{Ipv4Addr, SocketAddr, TcpStream};
        use std::time::Duration;
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, self.port));
        let stream =
            TcpStream::connect_timeout(&addr, Duration::from_secs(2)).map_err(|e| e.to_string())?;
        let timeout = Some(Duration::from_secs(if method == "POST" { 12 } else { 3 }));
        stream
            .set_read_timeout(timeout)
            .map_err(|e| e.to_string())?;
        stream
            .set_write_timeout(timeout)
            .map_err(|e| e.to_string())?;
        let mut stream = self
            .tls
            .connect("localhost", stream)
            .map_err(|e| e.to_string())?;
        let peer = stream
            .peer_certificate()
            .map_err(|e| e.to_string())?
            .ok_or("本地引擎未提供证书")?;
        if peer.to_der().map_err(|e| e.to_string())? != self.cert_der {
            return Err("本地端口的证书与此 FlashLink 主机不符".to_string());
        }
        let body = if body.is_null() {
            String::new()
        } else {
            body.to_string()
        };
        write!(stream,"{method} /api/pin HTTP/1.0\r\nHost: localhost:{}\r\nAuthorization: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",self.port,self.authorization,body.len()).map_err(|e|e.to_string())?;
        stream.flush().map_err(|e| e.to_string())?;
        let mut reader = BufReader::new(stream.take(96 * 1024));
        let mut line = String::new();
        reader.read_line(&mut line).map_err(|e| e.to_string())?;
        let status = line.split_whitespace().nth(1).unwrap_or("?");
        if status != "200" {
            return Err(format!("本机 Sunshine 管理接口返回 HTTP {status}"));
        }
        let mut length = None;
        loop {
            line.clear();
            if reader.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
                return Err("管理接口响应中断".into());
            }
            if line == "\r\n" {
                break;
            }
            if let Some((key, value)) = line.split_once(':') {
                if key.eq_ignore_ascii_case("Content-Length") {
                    length = value.trim().parse::<usize>().ok();
                }
            }
        }
        let len = length
            .filter(|v| *v <= 64 * 1024)
            .ok_or("管理接口响应长度异常")?;
        let mut json = vec![0; len];
        reader.read_exact(&mut json).map_err(|e| e.to_string())?;
        serde_json::from_slice(&json).map_err(|e| e.to_string())
    }

    pub fn pending(&self) -> Result<Vec<Pending>, String> {
        #[derive(serde::Deserialize)]
        struct Response {
            pairings: Vec<Pending>,
        }
        let response: Response =
            serde_json::from_value(self.request("GET", Value::Null)?).map_err(|e| e.to_string())?;
        Ok(response.pairings)
    }

    pub fn respond(&self, id: &str, allow: Option<(&str, &str)>) -> Result<bool, String> {
        let result = match allow {
            Some((pin, name)) => {
                self.request("POST", json!({"pairing_id":id,"pin":pin,"name":name}))?
            }
            None => self.request("DELETE", json!({"pairing_id":id}))?,
        };
        result["status"]
            .as_bool()
            .ok_or_else(|| "本机引擎未确认配对结果".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credentials_are_rotated_without_keeping_the_password_on_disk() {
        let path =
            std::env::temp_dir().join(format!("flashlink-auth-test-{}.json", std::process::id()));
        let first = Credentials::create(&path).unwrap();
        let second = Credentials::create(&path).unwrap();
        assert_ne!(first.authorization, second.authorization);
        let basic = STANDARD
            .decode(second.authorization.strip_prefix("Basic ").unwrap())
            .unwrap();
        let basic = String::from_utf8(basic).unwrap();
        let (_, password) = basic.split_once(':').unwrap();
        let file = fs::read_to_string(&path).unwrap();
        fs::remove_file(path).unwrap();
        let auth: serde_json::Value = serde_json::from_str(&file).unwrap();
        assert_eq!(
            auth["password"],
            sunshine_hash(password, auth["salt"].as_str().unwrap())
        );
        assert!(!file.contains(password));
    }

    #[test]
    fn reverse_sha256_is_the_format_used_by_sunshine() {
        assert_eq!(
            sunshine_hash("ab", "c"),
            "AD1500F261FF10B49C7A1796A36103B02322AE5DDE404141EACF018FBF1678BA"
        );
    }
}
