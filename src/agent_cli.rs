//! 이 머신에 설치된 에이전트 CLI(Claude Code `claude`, Codex `codex`) 찾기(ADR-0057).
//!
//! 앱은 Finder·시작 메뉴에서 열리면 셸의 `PATH`를 물려받지 못한다. 그래서 `PATH`에 더해 설치 프로그램이
//! 흔히 두는 곳도 살피고, 그래도 없으면 로그인 셸에 묻는다. `config.toml`에 경로(`claude_code_path`,
//! `codex_path`)가 있으면 그것만 본다.

use std::path::{Path, PathBuf};
use std::process::Command;

/// 찾을 CLI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cli {
    ClaudeCode,
    Codex,
}

impl Cli {
    /// 실행 파일 이름(확장자 없이).
    pub fn program(self) -> &'static str {
        match self {
            Cli::ClaudeCode => "claude",
            Cli::Codex => "codex",
        }
    }

    /// 화면에 보일 이름.
    pub fn title(self) -> &'static str {
        match self {
            Cli::ClaudeCode => "Claude Code",
            Cli::Codex => "Codex",
        }
    }

    /// 설치 프로그램이 흔히 두는 곳(홈 기준).
    fn home_dirs(self) -> &'static [&'static str] {
        match self {
            // 네이티브 설치(`claude install`), 예전 로컬 설치.
            Cli::ClaudeCode => &[".local/bin", ".claude/local"],
            Cli::Codex => &[".local/bin", ".codex/bin"],
        }
    }

    /// `config.toml`에 적은 경로.
    pub fn configured_path(self, config: &crate::config::Config) -> Option<PathBuf> {
        match self {
            Cli::ClaudeCode => config.claude_code_path.clone(),
            Cli::Codex => config.codex_path.clone(),
        }
    }
}

/// 찾은 CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Install {
    pub path: PathBuf,
    /// `--version`의 첫 줄(예: `2.1.0 (Claude Code)`). 실행하지 못하면 `None`.
    pub version: Option<String>,
}

/// CLI를 찾는다. `override_path`가 있으면 그 파일만 본다. 버전을 묻느라 프로세스를 띄우므로
/// 백그라운드에서 부른다.
pub fn detect(cli: Cli, override_path: Option<&Path>) -> Option<Install> {
    let path = find(cli, override_path)?;
    let version = version(&path);
    Some(Install { path, version })
}

/// 실행 파일만 찾는다(버전은 묻지 않는다). 로그인 셸에 물을 수 있어 백그라운드에서 부른다.
pub fn find(cli: Cli, override_path: Option<&Path>) -> Option<PathBuf> {
    match override_path {
        Some(path) => path.is_file().then(|| path.to_path_buf()),
        None => candidates(cli)
            .into_iter()
            .find(|path| path.is_file())
            .or_else(|| from_login_shell(cli)),
    }
}

/// 살필 곳: `PATH`의 디렉터리, 그다음 설치 프로그램이 흔히 두는 곳. 앞에 있을수록 먼저 쓴다.
fn candidates(cli: Cli) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    if let Some(home) = dirs::home_dir() {
        dirs.extend(cli.home_dirs().iter().map(|dir| home.join(dir)));
        // npm·bun·volta 전역 설치.
        dirs.push(home.join(".npm-global/bin"));
        dirs.push(home.join(".bun/bin"));
        dirs.push(home.join(".volta/bin"));
    }
    #[cfg(not(windows))]
    {
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
        dirs.push(PathBuf::from("/usr/local/bin"));
    }
    #[cfg(windows)]
    if let Some(appdata) = std::env::var_os("APPDATA") {
        dirs.push(PathBuf::from(appdata).join("npm"));
    }
    #[cfg(windows)]
    let names = [
        format!("{}.exe", cli.program()),
        format!("{}.cmd", cli.program()),
    ];
    #[cfg(not(windows))]
    let names = [cli.program().to_string()];
    dirs.iter()
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .collect()
}

/// 로그인 셸(`$SHELL`)이 아는 실행 파일. 셸 설정에서만 `PATH`에 더하는 곳(nvm 등)도 찾는다.
/// 대화형 셸이 다른 글을 찍을 수 있어 마지막 줄 중 있는 파일만 받는다.
#[cfg(not(windows))]
fn from_login_shell(cli: Cli) -> Option<PathBuf> {
    let shell = std::env::var_os("SHELL")?;
    let output = Command::new(shell)
        .args(["-l", "-i", "-c", &format!("command -v {}", cli.program())])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let path = PathBuf::from(stdout.lines().last()?.trim());
    (path.is_absolute() && path.is_file()).then_some(path)
}

#[cfg(windows)]
fn from_login_shell(_cli: Cli) -> Option<PathBuf> {
    None
}

/// `claude --version`의 첫 줄. npm 설치는 `node`가 `PATH`에 없으면 실패할 수 있다.
fn version(path: &Path) -> Option<String> {
    let mut command = Command::new(path);
    command.arg("--version");
    hide_console(&mut command);
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
}

/// Windows에서 콘솔 창이 잠깐 떴다 사라지지 않게 한다.
pub fn hide_console(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = command;
}
