//! 앱 테마 정의와 전환(ADR-0047).
//!
//! 테마는 두 곳에서 온다.
//! - 시스템 테마: `themes/hanji.json`(바이너리에 넣는다). 지울 수 없다.
//! - 라이브러리 테마: `<라이브러리>/Themes/*.json`. 사용자나 어시스턴트가 만들고, 설정 창에서 지운다.
//!
//! 두 곳 모두 gpui-component의 `ThemeSet` 스키마를 그대로 따른다. 앱 UI는 하드코딩된 색 대신
//! `cx.theme()` 토큰을 쓴다(ADR-0001).
//!
//! ```ignore
//! theme::register(cx);                  // 시스템 테마 등록
//! theme::load_library(Some(root), cx);  // 라이브러리 테마 (다시) 읽기
//! theme::choose("Hanji Dark", cx);     // 적용하고 기억
//! ```

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::component::{Theme, ThemeConfig, ThemeMode, ThemeSet};
use gpui_kit::gpui::{App, Global, SharedString};

use crate::config::Config;

/// 시스템 테마 세트 (gpui-component 테마 스키마).
const HANJI_THEMES: &str = include_str!("../themes/hanji.json");

/// 라이브러리 루트 안의 테마 폴더.
pub const THEMES_DIR: &str = "Themes";

/// 이보다 큰 테마 파일은 읽지 않는다.
const MAX_THEME_BYTES: u64 = 256 * 1024;

/// 기본 다크 테마 이름.
pub const DEFAULT_DARK: &str = "Hanji Dark";

/// 기본 라이트 테마 이름.
pub const DEFAULT_LIGHT: &str = "Hanji Light";

/// 라이브러리 테마 하나와 그 파일.
struct LibraryTheme {
    config: Rc<ThemeConfig>,
    file: PathBuf,
}

/// 쓸 수 있는 테마 전부. 메뉴·설정 창·전환이 이것만 본다.
#[derive(Default)]
struct ThemeStore {
    system: Vec<Rc<ThemeConfig>>,
    library: Vec<LibraryTheme>,
    /// 라이브러리 테마를 읽은 루트.
    root: Option<PathBuf>,
    /// 지난번에 읽은 (파일, 내용). 같으면 다시 적용하지 않는다.
    sources: Vec<(PathBuf, String)>,
}

impl Global for ThemeStore {}

impl ThemeStore {
    fn get(&self, name: &str) -> Option<&Rc<ThemeConfig>> {
        self.system
            .iter()
            .chain(self.library.iter().map(|theme| &theme.config))
            .find(|config| config.name == name)
    }
}

/// 시스템 테마를 등록한다. `gpui_kit::init` 이후에 호출해야 한다.
pub fn register(cx: &mut App) {
    let system = match parse(HANJI_THEMES) {
        Ok(themes) => themes.into_iter().map(Rc::new).collect(),
        Err(err) => {
            // 테마가 깨져도 앱 시작은 막지 않고 기본 테마로 계속 진행한다.
            eprintln!("테마 로드 실패: {err}");
            Vec::new()
        }
    };
    cx.set_global(ThemeStore {
        system,
        ..Default::default()
    });
}

/// 테마 파일 내용을 읽는다. 라이브러리 테마의 검사(어시스턴트의 `write_file`)에도 쓴다.
pub fn parse(content: &str) -> Result<Vec<ThemeConfig>, String> {
    let set: ThemeSet = serde_json::from_str(content).map_err(|err| err.to_string())?;
    if set.themes.is_empty() {
        return Err("\"themes\" has no theme.".into());
    }
    for theme in &set.themes {
        if theme.name.trim().is_empty() {
            return Err("A theme has no name.".into());
        }
    }
    Ok(set.themes)
}

/// 라이브러리 테마 파일 내용을 검사한다. 시스템 테마와 이름이 같으면 쓰지 않는다.
pub fn validate_library_file(content: &str) -> Result<(), String> {
    let system = parse(HANJI_THEMES).unwrap_or_default();
    for theme in parse(content)? {
        if system.iter().any(|builtin| builtin.name == theme.name) {
            return Err(format!(
                "\"{}\" is a built-in theme name. Pick another name.",
                theme.name
            ));
        }
    }
    Ok(())
}

/// `root`(없으면 라이브러리 없음)의 `Themes/*.json`을 다시 읽는다. 바뀐 것이 있으면 저장된 테마를
/// 다시 적용하고 `true`를 돌려준다.
///
/// 파일 이름순으로 읽고, 시스템 테마나 앞 파일의 테마와 이름이 같은 테마는 건너뛴다. 읽지 못하는
/// 파일도 건너뛴다. 서체·크기·모서리 값은 앱 레이아웃에 맞춰져 있으므로 버리고 색만 쓴다.
pub fn load_library(root: Option<&Path>, cx: &mut App) -> bool {
    let sources = root.map(read_sources).unwrap_or_default();
    let store = cx.global::<ThemeStore>();
    if store.root.as_deref() == root && store.sources == sources {
        return false;
    }
    let mut library: Vec<LibraryTheme> = Vec::new();
    for (file, content) in &sources {
        let themes = match parse(content) {
            Ok(themes) => themes,
            Err(err) => {
                eprintln!("테마 파일을 읽을 수 없습니다({}): {err}", file.display());
                continue;
            }
        };
        for mut theme in themes {
            let taken = store.system.iter().any(|config| config.name == theme.name)
                || library.iter().any(|other| other.config.name == theme.name);
            if taken {
                continue;
            }
            theme.is_default = false;
            theme.font_size = None;
            theme.font_family = None;
            theme.mono_font_family = None;
            theme.mono_font_size = None;
            theme.radius = None;
            theme.radius_lg = None;
            theme.shadow = None;
            library.push(LibraryTheme {
                config: Rc::new(theme),
                file: file.clone(),
            });
        }
    }
    let store = cx.global_mut::<ThemeStore>();
    store.library = library;
    store.root = root.map(Path::to_path_buf);
    store.sources = sources;
    apply_saved(cx);
    true
}

/// `root/Themes/`의 `.json` 파일 (경로, 내용). 이름순. 심볼릭 링크는 따라가지 않는다.
fn read_sources(root: &Path) -> Vec<(PathBuf, String)> {
    let dir = root.join(THEMES_DIR);
    if !std::fs::symlink_metadata(&dir).is_ok_and(|meta| meta.is_dir()) {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut sources: Vec<(PathBuf, String)> = entries
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            !name.starts_with('.') && name.to_ascii_lowercase().ends_with(".json")
        })
        .filter_map(|entry| {
            let path = entry.path();
            let meta = std::fs::symlink_metadata(&path).ok()?;
            if !meta.is_file() || meta.len() > MAX_THEME_BYTES {
                return None;
            }
            Some((path.clone(), std::fs::read_to_string(&path).ok()?))
        })
        .collect();
    sources.sort_by(|a, b| a.0.cmp(&b.0));
    sources
}

/// 저장된 테마(`config.toml`)를 적용한다. 없으면 지금 테마를 다시 적용하고(색이 바뀌었을 수 있다),
/// 그것도 없으면 같은 모드의 기본 테마로 간다. 저장된 이름은 바꾸지 않는다(라이브러리를 다시 열면 돌아온다).
pub fn apply_saved(cx: &mut App) {
    let saved = Config::load().theme.map(|name| renamed_builtin(&name));
    if saved.as_deref().is_some_and(|name| set_theme(name, cx)) {
        return;
    }
    let active = active_name(cx);
    if !set_theme(&active, cx) {
        set_theme(default_for(Theme::global(cx).mode), cx);
    }
}

/// 앱 이름이 Writer였을 때의 시스템 테마 이름을 지금 이름으로 바꾼다. 그 밖의 이름은 그대로 둔다.
fn renamed_builtin(name: &str) -> String {
    match name {
        "Writer Light" => DEFAULT_LIGHT.to_string(),
        "Writer Dark" => DEFAULT_DARK.to_string(),
        _ => name.to_string(),
    }
}

fn default_for(mode: ThemeMode) -> &'static str {
    if mode.is_dark() {
        DEFAULT_DARK
    } else {
        DEFAULT_LIGHT
    }
}

/// 메뉴·설정 창에 보일 테마 이름. 시스템 테마(파일 순서) 뒤에 라이브러리 테마(라이트 먼저, 이름순).
pub fn theme_names(cx: &App) -> Vec<SharedString> {
    let store = cx.global::<ThemeStore>();
    let mut library: Vec<&Rc<ThemeConfig>> =
        store.library.iter().map(|theme| &theme.config).collect();
    library.sort_by(|a, b| {
        a.mode
            .cmp(&b.mode)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    store
        .system
        .iter()
        .chain(library)
        .map(|config| config.name.clone())
        .collect()
}

/// 라이브러리 테마면 그 파일. 시스템 테마나 모르는 이름이면 `None`.
pub fn library_file(name: &str, cx: &App) -> Option<PathBuf> {
    cx.global::<ThemeStore>()
        .library
        .iter()
        .find(|theme| theme.config.name == name)
        .map(|theme| theme.file.clone())
}

/// 파일 `file`에 든 테마 이름들.
pub fn themes_in(file: &Path, cx: &App) -> Vec<SharedString> {
    cx.global::<ThemeStore>()
        .library
        .iter()
        .filter(|theme| theme.file == file)
        .map(|theme| theme.config.name.clone())
        .collect()
}

/// 이름으로 테마를 적용하고 성공 여부를 돌려준다. 모르는 이름이면 아무 것도 하지 않고 `false`.
///
/// 모드(라이트/다크)까지 함께 전환한다. 기억하지는 않는다([`choose`]).
pub fn set_theme(name: &str, cx: &mut App) -> bool {
    let Some(config) = cx.global::<ThemeStore>().get(name).cloned() else {
        return false;
    };
    Theme::update(cx, |theme| theme.apply_config(&config));
    true
}

/// 테마를 적용하고 다음 실행에 복원하도록 기억한다.
pub fn choose(name: &str, cx: &mut App) -> bool {
    if !set_theme(name, cx) {
        return false;
    }
    let mut config = Config::load();
    config.theme = Some(name.to_string());
    if let Err(err) = config.save() {
        eprintln!("설정 저장 실패: {err}");
    }
    true
}

/// 라이트/다크 전환 대상: 같은 이름 계열(`… Light` ↔ `… Dark`)의 반대 모드 테마, 없으면 기본 테마.
pub fn counterpart(cx: &App) -> SharedString {
    let theme = Theme::global(cx);
    let target = if theme.mode.is_dark() {
        ThemeMode::Light
    } else {
        ThemeMode::Dark
    };
    let store = cx.global::<ThemeStore>();
    let family = family(&active_name(cx)).to_string();
    store
        .system
        .iter()
        .chain(store.library.iter().map(|theme| &theme.config))
        .find(|config| config.mode == target && family_of(config) == family)
        .map(|config| config.name.clone())
        .unwrap_or_else(|| default_for(target).into())
}

fn family_of(config: &ThemeConfig) -> &str {
    family(&config.name)
}

/// 이름에서 끝의 ` Light`·` Dark`를 뗀 것.
fn family(name: &str) -> &str {
    name.strip_suffix(" Light")
        .or_else(|| name.strip_suffix(" Dark"))
        .unwrap_or(name)
}

/// 라이브러리 테마 파일을 휴지통으로 옮긴다. 지금 테마가 그 파일에 있으면 같은 모드의 기본 테마로 바꾼다.
pub fn delete_file(file: &Path, cx: &mut App) -> std::io::Result<()> {
    crate::library::move_to_trash(file)?;
    if library_file(&active_name(cx), cx).as_deref() == Some(file) {
        choose(default_for(Theme::global(cx).mode), cx);
    }
    let root = cx.global::<ThemeStore>().root.clone();
    load_library(root.as_deref(), cx);
    Ok(())
}

/// 현재 활성 테마 이름.
pub fn active_name(cx: &App) -> SharedString {
    Theme::global(cx).theme_name().clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_themes_parse() {
        let names: Vec<_> = parse(HANJI_THEMES)
            .expect("themes/hanji.json 파싱 실패")
            .into_iter()
            .map(|theme| theme.name)
            .collect();
        for name in [DEFAULT_LIGHT, DEFAULT_DARK, "Gruvbox Light", "Gruvbox Dark"] {
            assert!(names.iter().any(|n| n == name), "{name} 테마가 없습니다");
        }
    }

    #[test]
    fn library_file_cannot_reuse_system_names() {
        let theme = |name: &str| {
            format!(r#"{{"name":"x","themes":[{{"name":"{name}","mode":"dark","colors":{{}}}}]}}"#)
        };
        assert!(validate_library_file(&theme("Nord Dark")).is_ok());
        assert!(validate_library_file(&theme("Hanji Dark")).is_err());
        assert!(validate_library_file(r#"{"name":"x","themes":[]}"#).is_err());
        assert!(validate_library_file("{").is_err());
    }

    #[test]
    fn family_strips_mode_suffix() {
        assert_eq!(family("Gruvbox Dark"), "Gruvbox");
        assert_eq!(family("Gruvbox Light"), "Gruvbox");
        assert_eq!(family("Paper"), "Paper");
    }
}
