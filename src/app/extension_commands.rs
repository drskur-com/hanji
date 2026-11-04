//! 확장 명령(ADR-0026): `Extensions` 메뉴를 확장 폴더에 맞추고, 고른 명령을 열린 시트에 실행한다.
//!
//! 명령은 선택한 글과 시트 본문을 받아 글을 돌려준다. 돌려준 글로 선택 영역을 바꾼다(선택이 없으면
//! 커서 자리에 넣는다). 사용자가 직접 고른 일이고 실행 취소가 되므로 승인을 묻지 않는다.

use gpui_kit::gpui::*;

use super::HanjiApp;
use crate::RunExtensionCommand;
use crate::extension::{self, CommandContext, Extension};
use crate::menus::{ExtensionCommands, install_menus};

impl HanjiApp {
    /// 확장 폴더가 바뀌었으면 명령을 다시 읽어 메뉴를 다시 설치한다. 라이브러리가 없으면 비운다.
    pub(super) fn refresh_extension_commands(&mut self, cx: &mut Context<Self>) {
        let files = self.library.as_ref().map(|lib| extension::scan(&lib.root));
        if files == self.extension_files {
            return;
        }
        self.extension_files = files;
        let commands = self
            .library
            .as_ref()
            .map(|lib| extension::commands(&lib.root))
            .unwrap_or_default();
        let current = cx.try_global::<ExtensionCommands>().map(|global| &global.0);
        if current == Some(&commands) || (current.is_none() && commands.is_empty()) {
            return;
        }
        cx.set_global(ExtensionCommands(commands));
        install_menus(cx);
        #[cfg(not(target_os = "macos"))]
        self.menu_bar.update(cx, |menu_bar, cx| menu_bar.reload(cx));
    }

    pub(super) fn on_run_extension_command(
        &mut self,
        action: &RunExtensionCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.library.as_ref().map(|lib| lib.root.clone()) else {
            return;
        };
        let Some(editor) = &self.editor else {
            self.notify(t!("common.open_sheet_first"), window, cx);
            return;
        };
        let state = editor.state().clone();
        let sheet = editor
            .path()
            .strip_prefix(&root)
            .unwrap_or(editor.path())
            .to_string_lossy()
            .into_owned();
        let (text, range) = {
            let state = state.read(cx);
            let text = state.value().to_string();
            let range = state.selected_range();
            let range = range.start.min(text.len())..range.end.min(text.len());
            (text, range)
        };
        let ctx = CommandContext {
            selection: text.get(range.clone()).unwrap_or("").to_string(),
            text,
            sheet,
        };
        let result = Extension::open(&root, &action.extension)
            .and_then(|extension| extension.run_command(action.index, &ctx));
        let inserted = match result {
            Ok(Some(inserted)) => inserted,
            Ok(None) => return,
            Err(err) => {
                self.notify(t!("extensions.command_failed", err = err), window, cx);
                return;
            }
        };
        state.update(cx, |state, cx| {
            state.set_selected_range(range.clone(), cx);
            state.replace(inserted.clone(), window, cx);
            let end = range.start + inserted.len();
            state.set_selected_range(end..end, cx);
            state.focus(window, cx);
        });
        self.on_editor_changed(window, cx);
    }
}
