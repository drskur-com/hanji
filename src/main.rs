#![recursion_limit = "512"]
// 릴리스 빌드는 Windows GUI 서브시스템으로 — 없으면 실행할 때 콘솔 창이 함께 뜬다.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[macro_use]
extern crate rust_i18n;

// UI 문자열(ADR-0027). 없는 번역은 영어로 보인다.
i18n!("locales", fallback = "en");

#[cfg(not(target_os = "macos"))]
use gpui_kit::component::*;
use gpui_kit::gpui::*;

mod agent;
mod app;
mod assets;
mod aws_profiles;
mod cache;
mod config;
mod dictation;
mod dock_icon;
pub mod export;
mod extension;
mod gif;
mod i18n;
mod icons;
mod library;
mod markdown;
mod media;
mod menus;
#[cfg(any(target_os = "macos", target_os = "windows"))]
mod ocr;
mod settings;
mod text_transform;
mod theme;
mod thumbnail;
mod watch;
mod youtube_player;

use app::HanjiApp;
#[cfg(target_os = "macos")]
use app::style::{PANEL_INSET, TOOLBAR_HEIGHT};
use menus::install_menus;

actions!(
    writer,
    [
        NewSheet,
        SaveSheet,
        InsertImage,
        InsertVideo,
        InsertAudio,
        InsertFootnote,
        InsertTable,
        InsertOcrText,
        InsertExtension,
        InsertSpecialChar,
        SearchSheets,
        ManageExtensions,
        ToggleLineGroup,
        CompileProject,
        OpenDictation,
        OpenLibrary,
        CloseLibrary,
        OpenSettings,
        ToggleBold,
        ToggleItalic,
        InsertLink,
        FormatBody,
        FormatHeading1,
        FormatHeading2,
        FormatHeading3,
        FormatHeading4,
        FormatHeading5,
        FormatHeading6,
        FormatBulletList,
        FormatNumberedList,
        FormatQuote,
        ToggleSidebar,
        ToggleAssistant,
        TogglePreview,
        ToggleTheme,
        ShowAbout,
        CheckForUpdates,
        QuitApp
    ]
);

/// 이름을 가진 테마 전환 액션 (메뉴에서 동적으로 생성).
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = writer, no_json)]
struct SetTheme {
    name: SharedString,
}

/// 확장 명령 실행 액션(ADR-0026, `Extensions` 메뉴에서 동적으로 생성).
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = writer, no_json)]
struct RunExtensionCommand {
    extension: SharedString,
    index: usize,
}

const APP_NAME: &str = "Hanji";

/// 라이브러리를 열 때 빌트인 스킬을 `Skills/`에 깔거나 새 버전으로 바꾸고, `Extensions/`를 만든다.
/// 실패는 기록만 한다.
fn sync_builtin_skills(root: &std::path::Path) {
    if let Err(err) = agent::skills::sync_builtin(root) {
        eprintln!("빌트인 스킬 설치 실패: {err}");
    }
    // 모델이 확장 파일을 바로 쓸 수 있게 폴더를 만들어 둔다(ADR-0026).
    if let Err(err) = extension::ensure_dir(root) {
        eprintln!("확장 폴더를 만들 수 없습니다: {err}");
    }
}

fn main() {
    gpui_kit::application()
        .with_assets(assets::Assets)
        .run(|cx| {
            // 설정을 읽기 전에 예전 이름(Writer)의 설정·데이터·캐시 폴더를 옮긴다.
            config::migrate_legacy_dirs();
            gpui_kit::init(cx);
            // 웹 주소 이미지(`img(url)`, 이미지 삽입의 내려받기)에 쓰는 HTTP 클라이언트.
            match reqwest_client::ReqwestClient::user_agent(concat!(
                "Hanji/",
                env!("CARGO_PKG_VERSION")
            )) {
                Ok(client) => cx.set_http_client(std::sync::Arc::new(client)),
                Err(err) => eprintln!("HTTP 클라이언트를 만들 수 없습니다: {err}"),
            }
            dock_icon::install();
            // 확장 그림의 글자에 쓰는 시스템 서체를 미리 찾아 둔다(첫 그림이 기다리지 않게).
            extension::render::preload_fonts();

            // 테마 등록 후 저장된 테마(없거나 모르는 이름이면 기본 라이트) 적용.
            // 저장된 테마가 라이브러리 테마일 수 있어 마지막 라이브러리의 테마를 먼저 읽는다.
            let config = config::Config::load();
            theme::register(cx);
            theme::set_theme(theme::DEFAULT_LIGHT, cx);
            if !theme::load_library(config.library_root.as_deref(), cx) {
                theme::apply_saved(cx);
            }

            // 언어를 먼저 정해야 메뉴와 첫 화면이 그 언어로 나온다.
            settings::Prefs::init(&config, cx);
            // macOS: 네이티브 메뉴 / Windows·Linux: 인윈도우 AppMenuBar용 메뉴 등록
            install_menus(cx);
            // 설정 창은 어느 창에 포커스가 있든(또는 없든) 열 수 있어야 하므로 앱 전역에서 받는다.
            cx.on_action(|_: &OpenSettings, cx| settings::open(cx));

            // 최소 단축키
            cx.bind_keys([
                KeyBinding::new("ctrl-n", NewSheet, None),
                KeyBinding::new("ctrl-s", SaveSheet, None),
                KeyBinding::new("ctrl-o", OpenLibrary, None),
                KeyBinding::new("ctrl-,", OpenSettings, None),
                #[cfg(target_os = "macos")]
                KeyBinding::new("cmd-,", OpenSettings, None),
                KeyBinding::new("ctrl-b", ToggleBold, None),
                KeyBinding::new("ctrl-i", ToggleItalic, None),
                KeyBinding::new("ctrl-shift-b", ToggleSidebar, None),
                KeyBinding::new("ctrl-shift-l", ToggleTheme, None),
                #[cfg(target_os = "macos")]
                KeyBinding::new("cmd-b", ToggleBold, None),
                #[cfg(target_os = "macos")]
                KeyBinding::new("cmd-i", ToggleItalic, None),
                #[cfg(target_os = "macos")]
                KeyBinding::new("cmd-k", InsertLink, None),
                #[cfg(not(target_os = "macos"))]
                KeyBinding::new("ctrl-k", InsertLink, None),
                #[cfg(target_os = "macos")]
                KeyBinding::new("ctrl-cmd-s", ToggleSidebar, None),
                KeyBinding::new("cmd-shift-e", CompileProject, None),
                KeyBinding::new("ctrl-shift-e", CompileProject, None),
                #[cfg(target_os = "macos")]
                KeyBinding::new("ctrl-cmd-space", InsertSpecialChar, None),
                KeyBinding::new("ctrl-alt-space", InsertSpecialChar, None),
                // cmd-shift-f는 에디터의 바꾸기다.
                #[cfg(target_os = "macos")]
                KeyBinding::new("cmd-alt-f", SearchSheets, None),
                #[cfg(not(target_os = "macos"))]
                KeyBinding::new("ctrl-shift-f", SearchSheets, None),
            ]);

            // 저장된 라이브러리를 열어본다(없거나 실패하면 온보딩).
            let library = config
                .library_root
                .as_deref()
                .and_then(|root| library::open(root).ok())
                .inspect(|library| sync_builtin_skills(&library.root));
            let notice = if config.library_root.is_some() && library.is_none() {
                Some(SharedString::from(t!("library.notice.saved_unopenable")))
            } else {
                None
            };

            let window_bounds = Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(1180.0), px(740.0)),
                cx,
            )));

            // Windows/Linux: 네이티브 타이틀바를 숨기고 커스텀 타이틀바를 그린다.
            #[cfg(not(target_os = "macos"))]
            let options = WindowOptions {
                window_bounds,
                window_min_size: Some(size(px(760.0), px(420.0))),
                ..TitleBar::window_options()
            };

            // macOS: 투명 타이틀바. 신호등 버튼은 라이브러리 패널 위쪽 줄 안에 둔다.
            #[cfg(target_os = "macos")]
            let options = WindowOptions {
                window_bounds,
                window_min_size: Some(size(px(760.0), px(420.0))),
                titlebar: Some(TitlebarOptions {
                    title: Some(APP_NAME.into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(
                        px(PANEL_INSET + 12.0),
                        px(TOOLBAR_HEIGHT / 2.0 - 7.0),
                    )),
                }),
                ..Default::default()
            };

            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| HanjiApp::new(library, notice, window, cx))
            })
            .expect("창 열기에 실패했습니다");
        });
}
