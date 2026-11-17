//! Looking for a newer release on GitHub, and installing it where the program
//! can do that itself (the AppImage, the Windows installer, the macOS disk image).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

const LATEST: &str = "https://api.github.com/repos/Jorgen-VikingGod/HybridSliceCraft/releases/latest";
pub const RELEASES: &str = "https://github.com/Jorgen-VikingGod/HybridSliceCraft/releases/latest";

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    /// "0.16.0", without the leading v.
    pub version: String,
    /// The release page.
    pub page: String,
    pub notes: String,
    /// (file name, download address)
    pub assets: Vec<(String, String)>,
}

/// How this copy was installed, which decides how it is updated.
#[derive(Clone, Debug, PartialEq)]
pub enum Install {
    /// The AppImage file itself, replaced in place.
    AppImage(PathBuf),
    Flatpak,
    /// The .deb package; `apt` is true when the apt repository is set up.
    Deb { apt: bool },
    Windows,
    MacOs,
    /// Built from source or unpacked by hand.
    Other,
}

impl Install {
    pub fn detect() -> Install {
        if let Some(p) = std::env::var_os("APPIMAGE").map(PathBuf::from).filter(|p| p.is_file()) {
            return Install::AppImage(p);
        }
        if std::env::var_os("FLATPAK_ID").is_some() {
            return Install::Flatpak;
        }
        if cfg!(target_os = "windows") {
            return Install::Windows;
        }
        if cfg!(target_os = "macos") {
            return Install::MacOs;
        }
        let exe = std::env::current_exe().unwrap_or_default();
        if cfg!(target_os = "linux") && exe.starts_with("/usr") {
            let apt = std::fs::read_dir("/etc/apt/sources.list.d")
                .map(|d| {
                    d.flatten().any(|e| {
                        std::fs::read_to_string(e.path()).is_ok_and(|t| t.contains("jorgen-vikinggod.github.io/HybridSliceCraft"))
                    })
                })
                .unwrap_or(false);
            return Install::Deb { apt };
        }
        Install::Other
    }

    /// Whether "Install" can do the whole job from inside the program.
    pub fn installs_itself(&self) -> bool {
        matches!(self, Install::AppImage(_) | Install::Windows | Install::MacOs)
    }

    /// The release file for this kind of install and this machine.
    pub fn asset<'a>(&self, r: &'a Release) -> Option<&'a (String, String)> {
        let arch = std::env::consts::ARCH;
        let ends = match self {
            Install::AppImage(_) => format!("_{arch}.AppImage"),
            Install::Flatpak => format!("-{arch}.flatpak"),
            Install::Deb { .. } => match arch {
                "x86_64" => "_amd64.deb".into(),
                a => format!("_{a}.deb"),
            },
            Install::Windows => ".msi".into(),
            Install::MacOs => format!("{arch}-apple-darwin.dmg"),
            Install::Other => return None,
        };
        r.assets.iter().find(|(name, _)| name.ends_with(&ends))
    }
}

/// True when version `a` is newer than `b` ("0.15.10" > "0.15.9").
pub fn is_newer(a: &str, b: &str) -> bool {
    let parts = |v: &str| -> Vec<u64> {
        v.trim_start_matches('v')
            .split(['.', '-', '+'])
            .map_while(|p| p.parse().ok())
            .collect()
    };
    parts(a) > parts(b)
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(30)))
        .user_agent(concat!("HybridSliceCraft/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// The newest published release.
pub fn latest() -> Result<Release, String> {
    let text = agent()
        .get(LATEST)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    parse_release(&text)
}

fn parse_release(text: &str) -> Result<Release, String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let tag = v["tag_name"].as_str().ok_or("The answer has no version")?;
    let assets = v["assets"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| Some((x["name"].as_str()?.to_string(), x["browser_download_url"].as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default();
    Ok(Release {
        version: tag.trim_start_matches('v').to_string(),
        page: v["html_url"].as_str().unwrap_or(RELEASES).to_string(),
        notes: v["body"].as_str().unwrap_or_default().to_string(),
        assets,
    })
}

/// Download `url` to `dest`, counting progress in thousandths into `done`.
pub fn download(url: &str, dest: &Path, done: &Arc<AtomicU32>) -> Result<(), String> {
    let mut resp = agent().get(url).call().map_err(|e| e.to_string())?;
    let total = resp.body().content_length().unwrap_or(0);
    let mut body = resp.body_mut().with_config().limit(2 << 30).reader();
    let mut file = std::fs::File::create(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    let mut buf = vec![0u8; 1 << 16];
    let mut got = 0u64;
    loop {
        let n = body.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        got += n as u64;
        if total > 0 {
            done.store((got * 1000 / total).min(1000) as u32, Ordering::Relaxed);
        }
    }
    file.sync_all().map_err(|e| e.to_string())?;
    if total > 0 && got != total {
        return Err("The download stopped early.".into());
    }
    Ok(())
}

/// What happened after installing.
pub enum Installed {
    /// The new version is in place; start the program again to use it.
    Restart,
    /// An installer was started; close the program so it can replace it.
    Quit,
    /// The disk image was opened; the person drags the app across.
    Opened,
}

/// Fetch the release file and put it in place.
pub fn install(how: &Install, release: &Release, done: &Arc<AtomicU32>) -> Result<Installed, String> {
    let (name, url) = how.asset(release).ok_or("This release has no file for this computer.")?;
    match how {
        Install::AppImage(path) => {
            // Next to the old one, so the final rename stays on the same disk.
            let new = path.with_extension("AppImage.download");
            if let Err(e) = download(url, &new, done) {
                let _ = std::fs::remove_file(&new);
                return Err(e);
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
            }
            std::fs::rename(&new, path).map_err(|e| {
                let _ = std::fs::remove_file(&new);
                format!("{}: {e}", path.display())
            })?;
            Ok(Installed::Restart)
        }
        Install::Windows | Install::MacOs => {
            let file = std::env::temp_dir().join(name);
            download(url, &file, done)?;
            if matches!(how, Install::Windows) {
                std::process::Command::new("msiexec").arg("/i").arg(&file).spawn().map_err(|e| e.to_string())?;
                Ok(Installed::Quit)
            } else {
                std::process::Command::new("open").arg(&file).spawn().map_err(|e| e.to_string())?;
                Ok(Installed::Opened)
            }
        }
        _ => Err("This copy is updated outside the program.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_number() {
        assert!(is_newer("0.15.10", "0.15.9"));
        assert!(is_newer("v1.0.0", "0.99.99"));
        assert!(!is_newer("0.15.0", "0.15.0"));
        assert!(!is_newer("0.14.2", "0.15.0"));
    }

    #[test]
    fn picks_the_file_for_the_install() {
        let r = parse_release(
            r#"{"tag_name":"v0.16.0","html_url":"https://x/r","body":"notes","assets":[
            {"name":"hybridslicecraft_0.16.0_x86_64.AppImage","browser_download_url":"https://x/a"},
            {"name":"hybridslicecraft_0.16.0_x86_64.AppImage.zsync","browser_download_url":"https://x/z"},
            {"name":"hybridslicecraft_0.16.0_amd64.deb","browser_download_url":"https://x/d"},
            {"name":"hybridslicecraft_0.16.0_x64_en-US.msi","browser_download_url":"https://x/m"},
            {"name":"HybridSliceCraft_0.16.0_aarch64_aarch64-apple-darwin.dmg","browser_download_url":"https://x/arm"},
            {"name":"HybridSliceCraft-0.16.0-x86_64.flatpak","browser_download_url":"https://x/f"}]}"#,
        )
        .unwrap();
        assert_eq!(r.version, "0.16.0");
        if std::env::consts::ARCH == "x86_64" {
            assert_eq!(Install::AppImage(PathBuf::new()).asset(&r).unwrap().1, "https://x/a");
            assert_eq!(Install::Deb { apt: false }.asset(&r).unwrap().1, "https://x/d");
            assert_eq!(Install::Flatpak.asset(&r).unwrap().1, "https://x/f");
            assert!(Install::MacOs.asset(&r).is_none());
        }
        assert_eq!(Install::Windows.asset(&r).unwrap().1, "https://x/m");
        assert!(Install::Other.asset(&r).is_none());
    }

    /// Talks to GitHub and downloads the AppImage: `cargo test -- --ignored live_`.
    #[test]
    #[ignore]
    fn live_latest_and_appimage_replace() {
        let r = latest().unwrap();
        assert!(!r.version.is_empty() && !r.assets.is_empty(), "{r:?}");
        let dir = std::env::temp_dir().join("hsc-update-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("HybridSliceCraft.AppImage");
        std::fs::write(&path, b"old").unwrap();
        let done = Arc::new(AtomicU32::new(0));
        assert!(matches!(install(&Install::AppImage(path.clone()), &r, &done).unwrap(), Installed::Restart));
        assert_eq!(done.load(Ordering::Relaxed), 1000);
        let len = std::fs::metadata(&path).unwrap().len();
        assert!(len > 1_000_000, "{len}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
