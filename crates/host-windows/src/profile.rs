use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub const ENGINE_NAME: &str = "FlashLink";
const APPS: &str = r#"{"env":{},"apps":[{"name":"Desktop","image-path":"desktop.png"}]}"#;

pub struct Profile {
    pub config: PathBuf,
    pub log: PathBuf,
    pub auth: PathBuf,
    pub overrides: Vec<String>,
    _lock: File,
}

impl Profile {
    /// Sunshine opens its output with truncation. Keep the most recent log that actually
    /// contained a stream, including across Win+L and automatic engine restart. Moving the file
    /// leaves only one copy and preserves its timestamp for the local diagnostics collector.
    pub fn remember_stream_log(&self) {
        use std::io::{BufRead, BufReader};
        let Ok(log) = File::open(&self.log) else {
            return;
        };
        let active = BufReader::new(log)
            .split(b'\n')
            .map_while(Result::ok)
            .any(|line| {
                line.windows(b": Info: CLIENT CONNECTED".len())
                    .any(|s| s == b": Info: CLIENT CONNECTED")
            });
        if active {
            let _ = fs::rename(
                &self.log,
                self.log.with_file_name("flashlink-sunshine-previous.log"),
            );
        }
    }

    pub fn prepare(directory: &Path, port: u16) -> io::Result<Self> {
        let directory = std::path::absolute(directory)?;
        let root = directory.to_str().ok_or_else(|| invalid_path(&directory))?;
        // Sunshine treats '#' as a comment, and '=' in its config filename as a CLI setting.
        if root.contains(['#', '=', '\r', '\n']) {
            return Err(invalid_path(&directory));
        }
        fs::create_dir_all(&directory)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join("flashlink-demo.lock"))?;
        lock.try_lock().map_err(|e| {
            io::Error::other(format!(
                "cannot lock the FlashLink profile {}: {e}",
                directory.display()
            ))
        })?;
        // Keep the profile filenames shared with earlier versions and the same instance lock.
        let config = directory.join("flashlink-demo.conf");
        let auth = directory.join("flashlink-gui-auth.json");
        let log = directory.join("flashlink-sunshine.log");
        create_if_missing(
            &config,
            b"# FlashLink managed profile. The launcher supplies per-run settings.\n",
        )?;
        create_if_missing(&directory.join("flashlink-apps.json"), APPS.as_bytes())?;

        let mut overrides = vec![
            format!("port={port}"),
            format!("sunshine_name={ENGINE_NAME}"),
            format!("credentials_file={}", auth.display()),
            "origin_web_ui_allowed=pc".to_string(),
            "lan_encryption_mode=2".to_string(),
            "wan_encryption_mode=2".to_string(),
            "upnp=disabled".to_string(),
            "install_steam_audio_drivers=disabled".to_string(),
            "controller=disabled".to_string(),
            "gamepad_driver=vigembus".to_string(),
            "dd_configuration_option=disabled".to_string(),
            // Sunshine otherwise adjusts the machine's NVIDIA OpenGL/Vulkan swapchain policy.
            "nvenc_opengl_vulkan_on_dxgi=disabled".to_string(),
        ];
        // Sunshine resolves relative paths under its installation directory's config folder.
        // Keep state, identity and credentials under this Windows user's profile directory.
        for (key, name) in [
            ("file_apps", "flashlink-apps.json"),
            ("file_state", "flashlink-state.json"),
            ("pkey", "flashlink-key.pem"),
            ("cert", "flashlink-cert.pem"),
            ("log_path", "flashlink-sunshine.log"),
        ] {
            overrides.push(format!("{key}={}", directory.join(name).display()));
        }
        Ok(Self {
            config,
            log,
            auth,
            overrides,
            _lock: lock,
        })
    }
}

fn create_if_missing(path: &Path, contents: &[u8]) -> io::Result<()> {
    match File::create_new(path) {
        Ok(mut file) => file.write_all(contents),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e),
    }
}

fn invalid_path(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!(
            "Sunshine needs a Unicode profile path without '#', '=', or line breaks: {}",
            path.display()
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_retains_last_stream_diagnostics_without_overwriting_with_idle_startup() {
        let dir =
            std::env::temp_dir().join(format!("flashlink-log-restart-{}", std::process::id()));
        let profile = Profile::prepare(&dir, 47989).unwrap();
        let sample = b"[time]: Info: CLIENT CONNECTED\n[time]: Info: Opus initialized: 48 kHz\n";
        fs::write(&profile.log, sample).unwrap();
        profile.remember_stream_log();
        assert!(!profile.log.exists());
        fs::write(&profile.log, b"[time]: Info: idle\n").unwrap();
        profile.remember_stream_log();
        assert_eq!(
            fs::read(dir.join("flashlink-sunshine-previous.log")).unwrap(),
            sample
        );
        drop(profile);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn profile_uses_local_state_and_preserves_pairing_across_runs() {
        let root = std::env::temp_dir().join(format!(
            "flashlink-profile-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let profile = Profile::prepare(&root, 47989).unwrap();
        assert!(profile.config.is_file());
        assert!(profile
            .overrides
            .contains(&format!("credentials_file={}", profile.auth.display())));
        assert_eq!(profile.log, root.join("flashlink-sunshine.log"));
        assert!(profile
            .overrides
            .contains(&"lan_encryption_mode=2".to_owned()));
        assert!(profile
            .overrides
            .contains(&"origin_web_ui_allowed=pc".to_owned()));
        assert!(profile.overrides.contains(&format!(
            "file_state={}",
            root.join("flashlink-state.json").display()
        )));
        assert!(Profile::prepare(&root, 49000).is_err());
        fs::write(root.join("flashlink-state.json"), b"paired-device").unwrap();
        drop(profile);
        drop(Profile::prepare(&root, 47989).unwrap());
        assert_eq!(
            fs::read(root.join("flashlink-state.json")).unwrap(),
            b"paired-device"
        );
        fs::remove_dir_all(root).unwrap();
    }
}
