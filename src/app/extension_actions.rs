//! 확장 동작(ADR-0035): 블록의 영역을 누르면 `click`이 돌려준 동작을 앱이 실행한다.
//!
//! 블록 인자를 바꾸는 `set`은 에디터 블록이 바로 쓴다(`editor_extension.rs`). 여기서는 나머지(다른 확장의 명령,
//! 에디터에 글 넣기, 시트 열기, 메뉴)를 한다. 원고를 바꾸거나 다른 확장을 부르는 동작은 확장마다 처음 쓸 때
//! 허락을 받고, 허락은 라이브러리의 `.hanji/extensions.toml`에 둔다.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::dialog::DialogButtonProps;
use gpui_kit::gpui::*;

use super::HanjiApp;
use crate::extension::{Action, CommandRef, Extension};
use crate::{CompileProject, RunExtensionCommand, TogglePreview};

/// 허락 기록 파일(`.hanji/` 안).
const GRANTS_FILE: &str = "extensions.toml";

/// 확장마다 허락한 동작 이름.
#[derive(Default, serde::Deserialize, serde::Serialize)]
struct Grants {
    #[serde(default)]
    allowed: BTreeMap<String, BTreeSet<String>>,
}

fn grants_path(root: &Path) -> PathBuf {
    root.join(crate::library::MARKER_DIR).join(GRANTS_FILE)
}

fn read_grants(root: &Path) -> Grants {
    std::fs::read_to_string(grants_path(root))
        .ok()
        .and_then(|text| toml::from_str(&text).ok())
        .unwrap_or_default()
}

fn write_grants(root: &Path, grants: &Grants) -> std::io::Result<()> {
    let text = toml::to_string(grants).map_err(|err| std::io::Error::other(err.to_string()))?;
    let path = grants_path(root);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    crate::library::write_sheet(&path, &text).map(|_| ())
}

/// `actions` 중 `extension`이 아직 허락받지 못한 동작 이름.
fn missing_grants(root: &Path, extension: &str, actions: &[Action]) -> BTreeSet<String> {
    let grants = read_grants(root);
    let allowed = grants.allowed.get(extension);
    actions
        .iter()
        .filter(|action| action.needs_consent())
        .map(|action| action.name().to_string())
        .filter(|name| !allowed.is_some_and(|allowed| allowed.contains(name)))
        .collect()
}

impl HanjiApp {
    /// 확장 `extension`의 블록을 눌러 받은 동작을 차례로 실행한다. 허락이 필요한 동작이 처음이면 먼저 묻고,
    /// 허락하지 않으면 아무 동작도 하지 않는다.
    pub(super) fn run_extension_actions(
        &mut self,
        extension: &str,
        actions: Vec<Action>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.library.as_ref().map(|lib| lib.root.clone()) else {
            return;
        };
        let missing = missing_grants(&root, extension, &actions);
        if missing.is_empty() {
            for action in actions {
                self.run_extension_action(extension, action, window, cx);
            }
            return;
        }
        let lines: Vec<String> = actions
            .iter()
            .filter(|action| action.needs_consent())
            .map(|action| format!("• {}", action_label(action)))
            .collect();
        let description = format!(
            "{}\n\n{}",
            t!("extensions.consent_description", name = extension),
            lines.join("\n")
        );
        let app = cx.entity().downgrade();
        let extension = extension.to_string();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (app, extension, actions, missing, root) = (
                app.clone(),
                extension.clone(),
                actions.clone(),
                missing.clone(),
                root.clone(),
            );
            alert
                .confirm()
                .title(t!("extensions.consent_title"))
                .description(description.clone())
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t!("extensions.consent_allow"))
                        .cancel_text(t!("common.cancel")),
                )
                .on_ok(move |_, window, cx| {
                    let mut grants = read_grants(&root);
                    grants
                        .allowed
                        .entry(extension.clone())
                        .or_default()
                        .extend(missing.iter().cloned());
                    let saved = write_grants(&root, &grants);
                    let actions = actions.clone();
                    app.update(cx, |app, cx| {
                        if let Err(err) = saved {
                            app.notify(t!("extensions.consent_save_failed", err = err), window, cx);
                        }
                        for action in actions {
                            app.run_extension_action(&extension, action, window, cx);
                        }
                    })
                    .ok();
                    true
                })
        });
    }

    fn run_extension_action(
        &mut self,
        extension_name: &str,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            Action::Play(args) => self.play_extension(extension_name, args, cx),
            // 블록 인자는 에디터 블록이 이미 썼다.
            Action::Set { .. } => {}
            Action::Command { extension, command } => {
                let Some(root) = self.library.as_ref().map(|lib| lib.root.clone()) else {
                    return;
                };
                let index = match command {
                    CommandRef::Index(index) => Some(index),
                    CommandRef::Title(title) => Extension::open(&root, &extension)
                        .ok()
                        .and_then(|ext| ext.commands.iter().position(|t| *t == title)),
                };
                match index {
                    Some(index) => self.on_run_extension_command(
                        &RunExtensionCommand {
                            extension: extension.into(),
                            index,
                        },
                        window,
                        cx,
                    ),
                    None => self.notify(
                        t!(
                            "extensions.command_failed",
                            err = format!("{extension}: no such command")
                        ),
                        window,
                        cx,
                    ),
                }
            }
            Action::Insert(text) => self.insert_from_extension(&text, false, window, cx),
            Action::ReplaceSelection(text) => self.insert_from_extension(&text, true, window, cx),
            Action::Open(sheet) => {
                let Some(root) = self.library.as_ref().map(|lib| lib.root.clone()) else {
                    return;
                };
                let path = root.join(&sheet);
                // 라이브러리 밖이나 숨은 곳, 시트가 아닌 파일은 열지 않는다.
                let inside = std::path::Path::new(&sheet).components().all(|part| {
                    matches!(part, std::path::Component::Normal(name) if !name.to_string_lossy().starts_with('.'))
                });
                if inside && self.find_sheet(&path).is_some() {
                    self.open_linked_sheet(path, window, cx);
                } else {
                    self.notify(t!("extensions.open_not_found", sheet = sheet), window, cx);
                }
            }
            Action::Menu(menu) => match menu.as_str() {
                "preview" => self.on_toggle_preview(&TogglePreview, window, cx),
                "compile" => self.on_compile_project(&CompileProject, window, cx),
                _ => {}
            },
        }
    }

    /// 열린 시트의 커서 자리에 글을 넣는다(`replace`면 선택 영역을 바꾼다). 넣은 글 뒤에 커서를 둔다.
    fn insert_from_extension(
        &mut self,
        text: &str,
        replace: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = &self.editor else {
            return;
        };
        editor.state().update(cx, |state, cx| {
            let range = state.selected_range();
            let range = if replace { range } else { range.end..range.end };
            state.set_selected_range(range.clone(), cx);
            state.replace(text.to_string(), window, cx);
            let end = range.start + text.len();
            state.set_selected_range(end..end, cx);
        });
        self.on_editor_changed(window, cx);
    }
}

/// 허락 창에 보일 동작 설명(사용자의 언어).
fn action_label(action: &Action) -> String {
    match action {
        Action::Command {
            extension,
            command: CommandRef::Index(index),
        } => t!(
            "extensions.consent_command",
            name = extension,
            command = index + 1
        )
        .into(),
        Action::Command {
            extension,
            command: CommandRef::Title(title),
        } => t!(
            "extensions.consent_command",
            name = extension,
            command = title
        )
        .into(),
        Action::Insert(text) | Action::ReplaceSelection(text) => {
            let shown: String = text.chars().take(40).collect();
            let more = if text.chars().count() > 40 { "…" } else { "" };
            t!("extensions.consent_insert", text = format!("{shown}{more}")).into()
        }
        other => other.describe(),
    }
}

#[cfg(test)]
mod tests {
    use super::{Grants, missing_grants, read_grants, write_grants};
    use crate::extension::{Action, CommandRef};

    #[test]
    fn remembers_granted_actions_per_extension() {
        let root = std::env::temp_dir().join(format!("hanji-grants-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let actions = vec![
            Action::Set {
                block: "c".into(),
                args: Vec::new(),
            },
            Action::Insert("x".into()),
            Action::Command {
                extension: "other".into(),
                command: CommandRef::Index(0),
            },
            Action::Open("Notes/a.md".into()),
        ];
        // set·open은 묻지 않는다.
        let missing = missing_grants(&root, "buttons", &actions);
        assert_eq!(
            missing.iter().map(String::as_str).collect::<Vec<_>>(),
            ["command", "editor.insert"]
        );
        let mut grants = Grants::default();
        grants
            .allowed
            .entry("buttons".into())
            .or_default()
            .extend(missing);
        write_grants(&root, &grants).unwrap();
        assert!(missing_grants(&root, "buttons", &actions).is_empty());
        assert_eq!(missing_grants(&root, "other", &actions).len(), 2);
        assert_eq!(read_grants(&root).allowed.len(), 1);
        std::fs::remove_dir_all(&root).ok();
    }
}
