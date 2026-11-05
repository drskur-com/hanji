//! 플랫폼별 설치 위치 찾기·설치·다시 띄우기.

use std::path::{Path, PathBuf};
#[allow(unused_imports)]
use std::process::{Command, Stdio};

use crate::Error;

pub use imp::{Installation, installation};
pub(crate) use imp::{install, relaunch};

#[cfg(target_os = "macos")]
mod imp {
    use super::*;

    /// 지금 실행 중인 앱 번들과 그 서명.
    #[derive(Clone, Debug)]
    pub struct Installation {
        /// `…/Hanji.app`.
        pub app: PathBuf,
        /// 번들 ID(`com.drskur.hanji`).
        pub identifier: String,
        /// 서명한 Developer ID의 팀 ID.
        pub team_id: String,
    }

    /// 지금 앱이 스스로 업데이트할 수 있는 설치인지 확인한다.
    pub fn installation() -> Result<Installation, Error> {
        let exe = std::env::current_exe()?;
        let app = exe
            .ancestors()
            .nth(3)
            .filter(|path| path.extension().is_some_and(|ext| ext == "app"))
            .map(Path::to_path_buf)
            .ok_or_else(|| Error::Unsupported("Hanji is not running from an app bundle.".into()))?;
        // 다운로드 폴더나 dmg에서 바로 연 앱은 macOS가 읽기 전용 임시 위치(App Translocation)에서 띄운다.
        if app.to_string_lossy().contains("/AppTranslocation/") {
            return Err(Error::Unsupported(
                "Move Hanji to the Applications folder to get updates.".into(),
            ));
        }
        let (identifier, team_id) = signature(&app)?;
        let team_id = team_id.ok_or_else(|| {
            Error::Unsupported("This build is not signed with a Developer ID.".into())
        })?;
        Ok(Installation {
            app,
            identifier,
            team_id,
        })
    }

    /// (번들 ID, 팀 ID). ad-hoc 서명이면 팀 ID가 없다.
    fn signature(app: &Path) -> Result<(String, Option<String>), Error> {
        let output = Command::new("/usr/bin/codesign")
            .args(["-dv", "--verbose=2"])
            .arg(app)
            .output()?;
        // codesign은 이 정보를 stderr에 쓴다.
        let text = String::from_utf8_lossy(&output.stderr);
        if !output.status.success() {
            return Err(Error::Verification(format!(
                "Can't read the signature: {}",
                text.trim()
            )));
        }
        let field = |name: &str| {
            text.lines()
                .find_map(|line| line.strip_prefix(name))
                .map(str::trim)
                .map(str::to_string)
        };
        let identifier = field("Identifier=")
            .ok_or_else(|| Error::Verification("The app has no bundle identifier.".into()))?;
        let team_id = field("TeamIdentifier=").filter(|team| team != "not set");
        Ok((identifier, team_id))
    }

    fn succeeds(command: &mut Command) -> Result<bool, Error> {
        Ok(command
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?
            .success())
    }

    /// `app`이 지금 앱과 같은 번들 ID·팀 ID로 서명됐고, 서명이 온전하며, 공증됐는지.
    fn verify(installation: &Installation, app: &Path) -> Result<(), Error> {
        if !succeeds(
            Command::new("/usr/bin/codesign")
                .args(["--verify", "--deep", "--strict"])
                .arg(app),
        )? {
            return Err(Error::Verification(
                "The downloaded app's signature is broken.".into(),
            ));
        }
        let (identifier, team_id) = signature(app)?;
        if identifier != installation.identifier
            || team_id.as_deref() != Some(installation.team_id.as_str())
        {
            return Err(Error::Verification(format!(
                "The downloaded app is signed by someone else ({identifier}, {}).",
                team_id.as_deref().unwrap_or("no team")
            )));
        }
        if !succeeds(
            Command::new("/usr/sbin/spctl")
                .args(["--assess", "--type", "execute"])
                .arg(app),
        )? {
            return Err(Error::Verification(
                "The downloaded app is not notarized.".into(),
            ));
        }
        Ok(())
    }

    /// 마운트한 dmg. 버리면 떼어 낸다.
    struct Mount(PathBuf);

    impl Drop for Mount {
        fn drop(&mut self) {
            let _ = Command::new("/usr/bin/hdiutil")
                .args(["detach", "-force", "-quiet"])
                .arg(&self.0)
                .status();
            let _ = std::fs::remove_dir(&self.0);
        }
    }

    pub(crate) fn install(installation: &Installation, dmg: &Path) -> Result<(), Error> {
        let parent = installation
            .app
            .parent()
            .ok_or_else(|| Error::Unsupported("The app has no parent folder.".into()))?;
        let name = installation
            .app
            .file_name()
            .ok_or_else(|| Error::Unsupported("The app has no name.".into()))?
            .to_string_lossy()
            .into_owned();
        let staged = parent.join(format!(".{name}.update"));
        let old = parent.join(format!(".{name}.old"));
        for leftover in [&staged, &old] {
            if leftover.exists() {
                std::fs::remove_dir_all(leftover)?;
            }
        }
        // 쓰기 권한부터 본다(관리자가 아니면 /Applications에 못 쓸 수 있다).
        let probe = parent.join(format!(".{name}.probe"));
        std::fs::write(&probe, b"")
            .map_err(|_| Error::Unsupported(format!("Can't write to {}.", parent.display())))?;
        let _ = std::fs::remove_file(&probe);

        let mount_point = std::env::temp_dir()
            .join("hanji-update")
            .join(format!("mount-{}", std::process::id()));
        std::fs::create_dir_all(&mount_point)?;
        let attached = Command::new("/usr/bin/hdiutil")
            .args(["attach", "-nobrowse", "-readonly", "-noautoopen", "-quiet"])
            .arg("-mountpoint")
            .arg(&mount_point)
            .arg(dmg)
            .status()?;
        if !attached.success() {
            let _ = std::fs::remove_dir(&mount_point);
            return Err(Error::Io("Can't open the downloaded disk image.".into()));
        }
        let mount = Mount(mount_point);
        let source = std::fs::read_dir(&mount.0)?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| path.extension().is_some_and(|ext| ext == "app"))
            .ok_or_else(|| Error::InvalidRelease("The disk image has no app.".into()))?;
        verify(installation, &source)?;

        // ditto는 서명·확장 속성을 그대로 옮긴다.
        let copied = Command::new("/usr/bin/ditto")
            .arg(&source)
            .arg(&staged)
            .status()?;
        drop(mount);
        if !copied.success() {
            let _ = std::fs::remove_dir_all(&staged);
            return Err(Error::Io("Can't copy the new app.".into()));
        }
        if let Err(err) = verify(installation, &staged) {
            let _ = std::fs::remove_dir_all(&staged);
            return Err(err);
        }

        // 실행 중인 앱은 이름이 바뀌어도 계속 돈다. 바꾸다 실패하면 원래대로 돌려 둔다.
        std::fs::rename(&installation.app, &old)?;
        if let Err(err) = std::fs::rename(&staged, &installation.app) {
            let _ = std::fs::rename(&old, &installation.app);
            let _ = std::fs::remove_dir_all(&staged);
            return Err(err.into());
        }
        let _ = std::fs::remove_dir_all(&old);
        let _ = std::fs::remove_file(dmg);
        Ok(())
    }

    pub(crate) fn relaunch(installation: &Installation) -> Result<(), Error> {
        Command::new("/bin/sh")
            .arg("-c")
            .arg(r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done; /usr/bin/open "$2""#)
            .arg("sh")
            .arg(std::process::id().to_string())
            .arg(&installation.app)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(())
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use super::*;
    use std::os::windows::process::CommandExt as _;

    /// 콘솔 창을 띄우지 않는다.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    /// 지금 실행 중인 설치.
    #[derive(Clone, Debug)]
    pub struct Installation {
        /// `…\Hanji.exe`.
        pub exe: PathBuf,
    }

    pub fn installation() -> Result<Installation, Error> {
        let exe = std::env::current_exe()?;
        // 개발 중 빌드(`target\…`)는 설치된 앱이 아니다.
        if exe
            .components()
            .any(|component| component.as_os_str().eq_ignore_ascii_case("target"))
        {
            return Err(Error::Unsupported("Hanji is not installed.".into()));
        }
        Ok(Installation { exe })
    }

    /// PowerShell 작은따옴표 문자열.
    fn quote(path: &Path) -> String {
        format!("'{}'", path.to_string_lossy().replace('\'', "''"))
    }

    pub(crate) fn install(installation: &Installation, msi: &Path) -> Result<(), Error> {
        // 앱이 끝나야 파일을 바꿀 수 있으므로 기다렸다가 설치하고 다시 띄운다.
        let script = format!(
            "Wait-Process -Id {pid} -ErrorAction SilentlyContinue; \
             $p = Start-Process -FilePath msiexec.exe -ArgumentList @('/i', '\"' + {msi} + '\"', '/passive', '/norestart') -Wait -PassThru; \
             if ($p.ExitCode -eq 0 -or $p.ExitCode -eq 3010) {{ Remove-Item -LiteralPath {msi} -ErrorAction SilentlyContinue }}; \
             Start-Process -FilePath {exe}",
            pid = std::process::id(),
            msi = quote(msi),
            exe = quote(&installation.exe),
        );
        Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command"])
            .arg(script)
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(())
    }

    pub(crate) fn relaunch(_: &Installation) -> Result<(), Error> {
        Ok(())
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod imp {
    use super::*;

    #[derive(Clone, Debug)]
    pub struct Installation {
        pub exe: PathBuf,
    }

    pub fn installation() -> Result<Installation, Error> {
        Err(Error::Unsupported(
            "Automatic updates are not available on this platform.".into(),
        ))
    }

    pub(crate) fn install(_: &Installation, _: &Path) -> Result<(), Error> {
        installation().map(|_| ())
    }

    pub(crate) fn relaunch(_: &Installation) -> Result<(), Error> {
        installation().map(|_| ())
    }
}
