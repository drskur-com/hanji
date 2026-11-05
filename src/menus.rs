//! 앱 메뉴(macOS 네이티브 / 그 외 인윈도우 메뉴바).

use gpui_kit::component::*;
use gpui_kit::gpui::*;

#[cfg(target_os = "macos")]
use crate::APP_NAME;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use crate::InsertOcrText;
use crate::{
    CheckForUpdates, CloseLibrary, CompileProject, FormatBody, FormatBulletList, FormatHeading1,
    FormatHeading2, FormatHeading3, FormatHeading4, FormatHeading5, FormatHeading6,
    FormatNumberedList, FormatQuote, InsertAudio, InsertExtension, InsertFootnote, InsertImage,
    InsertLink, InsertSpecialChar, InsertTable, InsertVideo, ManageExtensions, NewSheet,
    OpenDictation, OpenLibrary, OpenSettings, QuitApp, RunExtensionCommand, SaveSheet,
    SearchSheets, SetTheme, ShowAbout, ToggleAssistant, ToggleBold, ToggleItalic, ToggleLineGroup,
    ToggleSidebar, theme,
};

/// 메뉴 트리.
///
/// - macOS: `Settings…`는 앱 메뉴(`Hanji`)에 둔다 (Apple HIG).
/// - 그 외: `Edit > Settings…`에 둔다.
/// - `Edit > Toggle Line Group`: 선택한 줄을 줄 묶음(`:::`)으로 감싸거나 푼다(ADR-0016).
/// - 테마 목록은 `ThemeRegistry`에서 읽어 `View > Theme`에 채운다.
/// - `Extensions` 메뉴(ADR-0026): 확장 명령을 확장 이름으로 묶고, 맨 아래에 `Manage Extensions…`를 둔다.
///   명령이 없어도 메뉴는 보인다.
pub(crate) fn build_menus(
    theme_names: &[SharedString],
    active_theme: &str,
    panels: PanelsVisible,
    extension_commands: &[(String, usize, String)],
) -> Vec<Menu> {
    let theme_items: Vec<MenuItem> = theme_names
        .iter()
        .map(|name| {
            let checked = name == active_theme;
            MenuItem::action(name.clone(), SetTheme { name: name.clone() }).checked(checked)
        })
        .collect();

    let mut menus: Vec<Menu> = Vec::new();

    #[cfg(target_os = "macos")]
    menus.push(Menu::new(APP_NAME).items([
        MenuItem::action(t!("menu.about"), ShowAbout),
        MenuItem::action(t!("menu.check_updates"), CheckForUpdates),
        MenuItem::separator(),
        MenuItem::action(t!("menu.settings"), OpenSettings),
        MenuItem::separator(),
        MenuItem::action(t!("menu.quit_app"), QuitApp),
    ]));

    let mut file_items = vec![
        MenuItem::action(t!("menu.new_sheet"), NewSheet),
        MenuItem::action(t!("menu.save"), SaveSheet),
        MenuItem::separator(),
        MenuItem::action(t!("menu.compile_project"), CompileProject),
        MenuItem::separator(),
    ];
    if crate::dictation::ENABLED {
        // 녹음해 라이브러리의 STT 폴더에 받아 적는 창(ADR-0054).
        file_items.push(MenuItem::action(t!("menu.dictation"), OpenDictation));
        file_items.push(MenuItem::separator());
    }
    file_items.push(MenuItem::action(t!("menu.open_library"), OpenLibrary));
    file_items.push(MenuItem::action(t!("menu.close_library"), CloseLibrary));
    #[cfg(not(target_os = "macos"))]
    {
        file_items.push(MenuItem::separator());
        file_items.push(MenuItem::action(t!("menu.quit"), QuitApp));
    }
    menus.push(Menu::new(t!("menu.file")).items(file_items));

    // `Emoji & Symbols…`는 macOS 관례상 Edit에, 그 밖에는 Insert > Special Characters… 하나로 둔다(같은 대화상자를 두 번 보이지 않게).
    let mut edit_items = vec![MenuItem::action(t!("menu.search_sheets"), SearchSheets)];
    #[cfg(target_os = "macos")]
    {
        edit_items.push(MenuItem::separator());
        edit_items.push(MenuItem::action(
            t!("menu.emoji_symbols"),
            InsertSpecialChar,
        ));
    }
    #[cfg(not(target_os = "macos"))]
    {
        edit_items.push(MenuItem::separator());
        edit_items.push(MenuItem::action(t!("menu.settings"), OpenSettings));
    }
    menus.push(Menu::new(t!("menu.edit")).items(edit_items));

    // 서식: 툴바에 있던 굵게·기울임·링크·문단 형식을 여기 모은다.
    menus.push(Menu::new(t!("menu.format")).items([
        MenuItem::action(t!("menu.bold"), ToggleBold),
        MenuItem::action(t!("menu.italic"), ToggleItalic),
        MenuItem::action(t!("menu.link"), InsertLink),
        MenuItem::separator(),
        MenuItem::action(t!("editor.format.body"), FormatBody),
        MenuItem::action(t!("editor.format.heading", level = 1), FormatHeading1),
        MenuItem::action(t!("editor.format.heading", level = 2), FormatHeading2),
        MenuItem::action(t!("editor.format.heading", level = 3), FormatHeading3),
        MenuItem::action(t!("editor.format.heading", level = 4), FormatHeading4),
        MenuItem::action(t!("editor.format.heading", level = 5), FormatHeading5),
        MenuItem::action(t!("editor.format.heading", level = 6), FormatHeading6),
        MenuItem::separator(),
        MenuItem::action(t!("editor.format.bullet_list"), FormatBulletList),
        MenuItem::action(t!("editor.format.numbered_list"), FormatNumberedList),
        MenuItem::action(t!("editor.format.quote"), FormatQuote),
        MenuItem::separator(),
        MenuItem::action(t!("menu.toggle_line_group"), ToggleLineGroup),
    ]));

    #[cfg_attr(
        not(any(target_os = "macos", target_os = "windows")),
        allow(unused_mut)
    )]
    let mut insert_items = vec![
        MenuItem::action(t!("menu.insert_image"), InsertImage),
        MenuItem::action(t!("menu.insert_video"), InsertVideo),
        MenuItem::action(t!("menu.insert_audio"), InsertAudio),
        MenuItem::action(t!("menu.insert_table"), InsertTable),
        MenuItem::action(t!("menu.insert_footnote"), InsertFootnote),
    ];
    // 이미지의 글자 인식은 운영체제의 인식기(macOS Vision, Windows.Media.Ocr)로 한다(ADR-0053).
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    insert_items.push(MenuItem::action(t!("menu.insert_ocr_text"), InsertOcrText));
    insert_items.extend([
        MenuItem::action(t!("menu.insert_extension"), InsertExtension),
        MenuItem::separator(),
        MenuItem::action(t!("menu.special_characters"), InsertSpecialChar),
    ]);
    menus.push(Menu::new(t!("menu.insert")).items(insert_items));

    {
        let mut items = Vec::new();
        let mut previous: Option<&str> = None;
        for (extension, index, title) in extension_commands {
            if previous.is_some_and(|previous| previous != extension) {
                items.push(MenuItem::separator());
            }
            previous = Some(extension);
            items.push(MenuItem::action(
                title.clone(),
                RunExtensionCommand {
                    extension: extension.clone().into(),
                    index: *index,
                },
            ));
        }
        if !items.is_empty() {
            items.push(MenuItem::separator());
        }
        items.push(MenuItem::action(
            t!("menu.manage_extensions"),
            ManageExtensions,
        ));
        menus.push(Menu::new(t!("menu.extensions")).items(items));
    }

    let sidebar_label = if panels.sidebar {
        t!("menu.hide_sidebar")
    } else {
        t!("menu.show_sidebar")
    };
    let assistant_label = if panels.assistant {
        t!("menu.hide_assistant")
    } else {
        t!("menu.show_assistant")
    };
    let mut view_items = vec![
        MenuItem::action(sidebar_label, ToggleSidebar),
        MenuItem::action(assistant_label, ToggleAssistant),
    ];
    if !theme_items.is_empty() {
        view_items.push(MenuItem::separator());
        view_items.push(MenuItem::submenu(
            Menu::new(t!("menu.theme")).items(theme_items),
        ));
    }
    menus.push(Menu::new(t!("menu.view")).items(view_items));

    menus.push(Menu::new(t!("menu.help")).items([
        MenuItem::action(t!("menu.check_updates"), CheckForUpdates),
        MenuItem::separator(),
        MenuItem::action(t!("menu.about"), ShowAbout),
    ]));

    menus
}

/// 열린 라이브러리의 확장 명령: (확장 이름, 명령 번호, 제목). 메뉴를 만들 때 읽는다.
#[derive(Default)]
pub(crate) struct ExtensionCommands(pub(crate) Vec<(String, usize, String)>);

impl Global for ExtensionCommands {}

/// 사이드바(보관함)와 AI 어시스턴트 패널이 보이는지. `View`의 보기/가리기 항목 이름을 정할 때 읽는다.
#[derive(Clone, Copy)]
pub(crate) struct PanelsVisible {
    pub(crate) sidebar: bool,
    pub(crate) assistant: bool,
}

impl Default for PanelsVisible {
    fn default() -> Self {
        Self {
            sidebar: true,
            assistant: false,
        }
    }
}

impl Global for PanelsVisible {}

/// 현재 테마, 패널 상태, 확장 명령을 반영해 앱 메뉴를 (재)설치한다.
pub(crate) fn install_menus(cx: &mut App) {
    let theme_names = theme::theme_names(cx);
    let active = theme::active_name(cx);
    let commands = cx
        .try_global::<ExtensionCommands>()
        .map(|commands| commands.0.clone())
        .unwrap_or_default();
    let panels = cx
        .try_global::<PanelsVisible>()
        .copied()
        .unwrap_or_default();

    cx.set_menus(build_menus(&theme_names, &active, panels, &commands));
    let owned: Vec<OwnedMenu> = build_menus(&theme_names, &active, panels, &commands)
        .into_iter()
        .map(|menu| menu.owned())
        .collect();
    GlobalState::global_mut(cx).set_app_menus(owned);
}
