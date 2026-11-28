//! The downloadable GUI executable has the verified engine ZIP, a text notice and a fixed
//! footer after the PE image. The Windows loader ignores the overlay; no external installer runs.
use sha2::{Digest, Sha256};
use std::os::windows::process::CommandExt;
use std::{
    env, fs,
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::Command,
};

const MAGIC: &[u8; 16] = b"FLASHLINK-PACK-1";
// bundle.py binds the two hashes into the host at compile time. Unpacked
// developer builds retain support for the original upstream release.
const ZIP_HASH: &str = match option_env!("FLASHLINK_ENGINE_ZIP_SHA256") {
    Some(value) => value,
    None => "233008e46f4c0e501a586cbfd6c4fd4a4c0d414a0b5fc7f13c070eb92ec3824b",
};
const EXE_HASH: &str = match option_env!("FLASHLINK_ENGINE_EXE_SHA256") {
    Some(value) => value,
    None => "a8ddf74e93297529ca5aa9c59234a7043b94587e8d11ea24c521fa69d6f59fd3",
};

struct Bundle {
    file: fs::File,
    zip_offset: u64,
    zip_len: u64,
    notice_len: u64,
}
impl Bundle {
    fn open(path: &Path) -> io::Result<Option<Self>> {
        let mut file = fs::File::open(path)?;
        let total = file.metadata()?.len();
        if total < 32 {
            return Ok(None);
        }
        file.seek(SeekFrom::End(-32))?;
        let mut footer = [0; 32];
        file.read_exact(&mut footer)?;
        if &footer[16..] != MAGIC {
            return Ok(None);
        }
        let size = u64::from_le_bytes(footer[..8].try_into().unwrap());
        let notice = u64::from_le_bytes(footer[8..16].try_into().unwrap());
        let expected_size = option_env!("FLASHLINK_ENGINE_ZIP_SIZE")
            .unwrap_or("38060196")
            .parse::<u64>()
            .map_err(io::Error::other)?;
        if size != expected_size
            || size > 256 * 1024 * 1024
            || notice > 16 * 1024 * 1024
            || total <= 32 + size + notice
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "离线包长度无效，请重新下载",
            ));
        }
        Ok(Some(Self {
            file,
            zip_offset: total - 32 - size - notice,
            zip_len: size,
            notice_len: notice,
        }))
    }
}

fn appdata() -> io::Result<PathBuf> {
    env::var_os("LOCALAPPDATA")
        .map(|d| PathBuf::from(d).join("FlashLink"))
        .ok_or_else(|| io::Error::other("缺少当前用户的本地目录"))
}
fn nonce() -> io::Result<String> {
    let mut bytes = [0u8; 12];
    getrandom::fill(&mut bytes).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
fn hash_file(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    io::copy(&mut file, &mut hash)?;
    Ok(format!("{:x}", hash.finalize()))
}

pub fn write_pure_executable(dest: &Path) -> io::Result<()> {
    let current = env::current_exe()?;
    let length = match Bundle::open(&current)? {
        Some(bundle) => Some(bundle.zip_offset),
        None => None,
    };
    let parent = dest
        .parent()
        .ok_or_else(|| io::Error::other("目标可执行文件路径无效"))?;
    let temp = parent.join(format!("helper-{}.tmp", nonce()?));
    let mut src = fs::File::open(&current)?;
    let write_res = (|| -> io::Result<()> {
        let mut out = fs::File::create_new(&temp)?;
        match length {
            Some(limit) => {
                let copied = io::copy(&mut (&mut src).take(limit), &mut out)?;
                if copied != limit {
                    return Err(io::Error::other("复制主机可执行文件不完整"));
                }
            }
            None => {
                io::copy(&mut src, &mut out)?;
            }
        }
        out.sync_all()
    })();
    if let Err(e) = write_res {
        let _ = fs::remove_file(&temp);
        return Err(e);
    }
    let mut last_err = None;
    for _ in 0..20 {
        match fs::rename(&temp, dest) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last_err = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    }
    let _ = fs::remove_file(&temp);
    Err(last_err.unwrap_or_else(|| io::Error::other("替换虚拟屏助手程序失败")))
}

pub fn relaunch_installed() -> io::Result<bool> {
    let current = env::current_exe()?;
    if Bundle::open(&current)?.is_none() {
        return Ok(false);
    }
    let dir = appdata()?.join("Application");
    fs::create_dir_all(&dir)?;
    let installed = dir.join("FlashLink.exe");
    if installed.is_file() && fs::canonicalize(&installed)? == fs::canonicalize(&current)? {
        return Ok(false);
    }
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("install.lock"))?;
    lock.lock()?;
    if !installed.is_file() || hash_file(&installed)? != hash_file(&current)? {
        let tmp = dir.join(format!("update-{}.exe", nonce()?));
        if let Err(e) = fs::copy(&current, &tmp).and_then(|_| fs::rename(&tmp, &installed)) {
            let _ = fs::remove_file(tmp);
            return Err(io::Error::other(format!(
                "更新本机主程序失败。请先关闭已运行的 FlashLink：{e}"
            )));
        }
    }
    Command::new(installed)
        .args(env::args_os().skip(1))
        .spawn()?;
    Ok(true)
}

pub fn prepare() -> io::Result<Option<PathBuf>> {
    let Some(mut bundle) = Bundle::open(&env::current_exe()?)? else {
        return Ok(None);
    };
    let root = appdata()?.join("Engines");
    fs::create_dir_all(&root)?;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("unpack.lock"))?;
    lock.lock()?;
    let installed = root.join(format!("Sunshine-{ZIP_HASH}"));
    let exe = installed.join("Sunshine").join("sunshine.exe");
    let notices = appdata()?.join("Application").join("OpenSourceNotices.txt");
    bundle
        .file
        .seek(SeekFrom::Start(bundle.zip_offset + bundle.zip_len))?;
    let mut text = Vec::with_capacity(bundle.notice_len as usize);
    (&mut bundle.file)
        .take(bundle.notice_len)
        .read_to_end(&mut text)?;
    fs::write(notices, text)?;
    if exe.is_file()
        && hash_file(&exe)? == EXE_HASH
        && installed.join("Sunshine/assets/web/index.html").is_file()
    {
        return Ok(Some(exe));
    }
    let token = nonce()?;
    let stage = root.join(format!("unpack-{token}"));
    fs::create_dir(&stage)?;
    let result = (|| {
        bundle.file.seek(SeekFrom::Start(bundle.zip_offset))?;
        let archive = stage.join("engine.zip");
        let mut out = fs::File::create_new(&archive)?;
        let len = io::copy(&mut (&mut bundle.file).take(bundle.zip_len), &mut out)?;
        out.flush()?;
        drop(out);
        if len != bundle.zip_len || hash_file(&archive)? != ZIP_HASH {
            return Err(io::Error::other(
                "内置 Sunshine 的 SHA-256 校验失败，请重新下载 FlashLink。",
            ));
        }
        let unpack = stage.join("runtime");
        fs::create_dir(&unpack)?;
        let tar = PathBuf::from(env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into()))
            .join(r"System32\tar.exe");
        let status = Command::new(tar)
            .arg("-xf")
            .arg(archive)
            .arg("-C")
            .arg(&unpack)
            .creation_flags(0x08000000)
            .status()?;
        if !status.success() || hash_file(&unpack.join("Sunshine/sunshine.exe"))? != EXE_HASH {
            return Err(io::Error::other("无法解压或验证内置 Sunshine。"));
        }
        if installed.try_exists()? {
            let previous = root.join(format!("previous-runtime-{token}"));
            fs::rename(&installed, &previous)?;
            if let Err(error) = fs::rename(&unpack, &installed) {
                fs::rename(&previous, &installed).map_err(|restore_error| {
                    io::Error::other(format!(
                        "替换引擎失败：{error}。原引擎保存在 {}，恢复失败：{restore_error}",
                        previous.display()
                    ))
                })?;
                return Err(error);
            }
        } else {
            fs::rename(unpack, &installed)?;
        }
        Ok(Some(exe))
    })();
    let _ = fs::remove_dir_all(stage);
    result
}
