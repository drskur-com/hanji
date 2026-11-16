//! 단축키 표. 앱이 묶는 단축키는 여기 한 곳에 두고, 설정 창의 "단축키" 페이지가 같은 표를 보인다.
//!
//! 에디터의 기본 편집 키(되돌리기·찾기 등)는 gpui-base가 `Input` 문맥에 묶으므로, 표에는 보이기만 하는
//! 항목으로 적는다. gpui-base의 키를 바꾸면 여기도 맞춘다.

use gpui_kit::gpui::KeyBinding;

use crate::{
    CompileProject, InsertLink, InsertSpecialChar, NewSheet, OpenLibrary, OpenSettings, SaveSheet,
    SearchSheets, ToggleBold, ToggleItalic, ToggleSidebar, ToggleTheme,
};

/// 단축키 묶음(설정 화면의 소제목).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Category {
    General,
    Format,
    Editor,
}

impl Category {
    pub const ALL: [Category; 3] = [Category::General, Category::Format, Category::Editor];

    pub fn title(self) -> String {
        match self {
            Category::General => t!("settings.shortcuts.section.general"),
            Category::Format => t!("settings.shortcuts.section.format"),
            Category::Editor => t!("settings.shortcuts.section.editor"),
        }
        .to_string()
    }
}

/// 단축키 한 줄.
pub struct Shortcut {
    pub category: Category,
    /// 이름의 번역 키.
    label: &'static str,
    /// 키(gpui 키 표기). 첫 번째를 화면에 보이고, 앱이 묶는 항목은 모두 묶는다.
    pub keys: &'static [&'static str],
    /// 앱이 묶는 동작. `None`이면 에디터가 이미 묶은 키를 보이기만 한다.
    bind: Option<fn(&str) -> KeyBinding>,
}

impl Shortcut {
    pub fn label(&self) -> String {
        t!(self.label).to_string()
    }
}

const MAC: bool = cfg!(target_os = "macos");

/// 플랫폼에 따라 다른 키.
const fn keys(
    mac: &'static [&'static str],
    other: &'static [&'static str],
) -> &'static [&'static str] {
    if MAC { mac } else { other }
}

/// 모든 단축키(화면 순서).
pub fn all() -> Vec<Shortcut> {
    fn app(
        category: Category,
        label: &'static str,
        keys: &'static [&'static str],
        bind: fn(&str) -> KeyBinding,
    ) -> Shortcut {
        Shortcut {
            category,
            label,
            keys,
            bind: Some(bind),
        }
    }
    fn editor(label: &'static str, keys: &'static [&'static str]) -> Shortcut {
        Shortcut {
            category: Category::Editor,
            label,
            keys,
            bind: None,
        }
    }
    use Category::*;
    vec![
        app(General, "settings.shortcuts.new_sheet", &["ctrl-n"], |k| {
            KeyBinding::new(k, NewSheet, None)
        }),
        app(General, "settings.shortcuts.save", &["ctrl-s"], |k| {
            KeyBinding::new(k, SaveSheet, None)
        }),
        app(
            General,
            "settings.shortcuts.open_library",
            &["ctrl-o"],
            |k| KeyBinding::new(k, OpenLibrary, None),
        ),
        app(
            General,
            "settings.shortcuts.search_sheets",
            // cmd-shift-f는 에디터의 바꾸기다.
            keys(&["cmd-alt-f"], &["ctrl-shift-f"]),
            |k| KeyBinding::new(k, SearchSheets, None),
        ),
        app(
            General,
            "settings.shortcuts.compile_project",
            keys(
                &["cmd-shift-e", "ctrl-shift-e"],
                &["ctrl-shift-e", "cmd-shift-e"],
            ),
            |k| KeyBinding::new(k, CompileProject, None),
        ),
        app(
            General,
            "settings.shortcuts.toggle_sidebar",
            keys(&["ctrl-cmd-s", "ctrl-shift-b"], &["ctrl-shift-b"]),
            |k| KeyBinding::new(k, ToggleSidebar, None),
        ),
        app(
            General,
            "settings.shortcuts.toggle_theme",
            &["ctrl-shift-l"],
            |k| KeyBinding::new(k, ToggleTheme, None),
        ),
        app(
            General,
            "settings.shortcuts.settings",
            keys(&["cmd-,", "ctrl-,"], &["ctrl-,"]),
            |k| KeyBinding::new(k, OpenSettings, None),
        ),
        app(
            Format,
            "settings.shortcuts.bold",
            keys(&["cmd-b", "ctrl-b"], &["ctrl-b"]),
            |k| KeyBinding::new(k, ToggleBold, None),
        ),
        app(
            Format,
            "settings.shortcuts.italic",
            keys(&["cmd-i", "ctrl-i"], &["ctrl-i"]),
            |k| KeyBinding::new(k, ToggleItalic, None),
        ),
        app(
            Format,
            "settings.shortcuts.link",
            keys(&["cmd-k"], &["ctrl-k"]),
            |k| KeyBinding::new(k, InsertLink, None),
        ),
        app(
            Format,
            "settings.shortcuts.special_characters",
            keys(&["ctrl-cmd-space", "ctrl-alt-space"], &["ctrl-alt-space"]),
            |k| KeyBinding::new(k, InsertSpecialChar, None),
        ),
        editor("settings.shortcuts.undo", keys(&["cmd-z"], &["ctrl-z"])),
        editor(
            "settings.shortcuts.redo",
            keys(&["cmd-shift-z"], &["ctrl-y"]),
        ),
        editor("settings.shortcuts.cut", keys(&["cmd-x"], &["ctrl-x"])),
        editor("settings.shortcuts.copy", keys(&["cmd-c"], &["ctrl-c"])),
        editor("settings.shortcuts.paste", keys(&["cmd-v"], &["ctrl-v"])),
        editor(
            "settings.shortcuts.select_all",
            keys(&["cmd-a"], &["ctrl-a"]),
        ),
        editor("settings.shortcuts.find", keys(&["cmd-f"], &["ctrl-f"])),
        editor(
            "settings.shortcuts.replace",
            keys(&["cmd-shift-f"], &["ctrl-h"]),
        ),
        editor("settings.shortcuts.indent", keys(&["cmd-]"], &["ctrl-]"])),
        editor("settings.shortcuts.outdent", keys(&["cmd-["], &["ctrl-["])),
    ]
}

/// 앱이 묶을 키(전역 문맥).
pub fn bindings() -> Vec<KeyBinding> {
    all()
        .iter()
        .filter_map(|shortcut| Some((shortcut.bind?, shortcut.keys)))
        .flat_map(|(bind, keys)| keys.iter().map(move |key| bind(key)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_parses() {
        for shortcut in all() {
            assert!(!shortcut.keys.is_empty());
            for key in shortcut.keys {
                assert!(
                    gpui_kit::gpui::Keystroke::parse(key).is_ok(),
                    "잘못된 키: {key}"
                );
            }
        }
    }
}
