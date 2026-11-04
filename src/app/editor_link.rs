//! 에디터 링크 열기: 보조 키(macOS ⌘, Windows·Linux Ctrl)를 누르고 링크를 누르면 연다.
//! 그냥 클릭은 지금처럼 커서를 놓아 링크 글을 고친다.
//!
//! - 주소(`https:`·`mailto:` 등 스킴이 있는 것) → 시스템 기본 앱
//! - 상대·절대 경로 → 시트 폴더 기준으로 찾아(이미지와 같은 규칙), 라이브러리의 시트면 앱에서 열고
//!   그 밖의 파일은 시스템 기본 앱으로 연다
//! - `#제목`(문서 안 이동)은 아직 없다
//!
//! 링크가 아니고 확장 블록 안이면 블록 편집 대화상자를 연다.

use std::path::{Path, PathBuf};

use gpui_kit::component::input::EditorState;
use gpui_kit::gpui::*;

use super::{HanjiApp, Selection};
use crate::{library, markdown};

/// 에디터에 링크 열기를 단다.
pub(super) fn install(state: &mut EditorState, app: WeakEntity<HanjiApp>) {
    state.set_secondary_click(move |text, offset, window, cx| {
        let Some(url) = markdown::link_at(text, offset) else {
            // 확장 블록(펼친 펜스 글)이면 블록 편집 대화상자를 연다(ADR-0033).
            let row = text[..offset.min(text.len())].matches('\n').count();
            if !crate::extension::blocks(text)
                .iter()
                .any(|block| block.rows.contains(&row))
            {
                return false;
            }
            let app = app.clone();
            window.defer(cx, move |window, cx| {
                app.update(cx, |app, cx| app.edit_extension_block(row, window, cx))
                    .ok();
            });
            return true;
        };
        // 에디터를 고치는 중이라 시트 열기(에디터 교체)는 이 처리가 끝난 뒤에 한다.
        let app = app.clone();
        window.defer(cx, move |window, cx| {
            app.update(cx, |app, cx| app.open_link(&url, window, cx))
                .ok();
        });
        true
    });
}

impl HanjiApp {
    /// 링크 주소를 연다. 상대 경로는 열린 시트의 폴더 기준이다.
    fn open_link(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        if has_scheme(url) {
            cx.open_url(url);
            return;
        }
        let target = percent_decode(url.split(['#', '?']).next().unwrap_or(url));
        if target.is_empty() {
            return;
        }
        let Some(dir) = self
            .editor
            .as_ref()
            .and_then(|editor| editor.path().parent().map(Path::to_path_buf))
        else {
            return;
        };
        let Some(path) = library::resolve_image_path(&dir, None, &target) else {
            self.notify(t!("editor.link.target_not_found", url = url), window, cx);
            return;
        };
        let in_library = self
            .library
            .as_ref()
            .is_some_and(|lib| path.starts_with(&lib.root));
        let markdown = path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));
        if in_library && markdown {
            self.open_linked_sheet(path, window, cx);
        } else {
            cx.open_with_system(&path);
        }
    }

    /// 라이브러리의 시트를 연다. 다른 프로젝트·메모의 시트면 그 화면과 그룹으로 옮겨 간다.
    pub(super) fn open_linked_sheet(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let project = self.library.as_ref().and_then(|lib| lib.project_of(&path));
        match &project {
            Some(project) if self.open_project.as_ref() != Some(project) => {
                self.enter_project(project, window, cx);
            }
            None if self.open_project.is_some() => self.leave_project(window, cx),
            _ => {}
        }
        if let Some(group) = path.parent().filter(|dir| self.find_group(dir).is_some()) {
            self.selected = Some(Selection::Group(group.to_path_buf()));
            self.selected_groups.clear();
            self.group_anchor = Some(group.to_path_buf());
        }
        self.select_sheet(Some(path), true, window, cx);
        self.load_previews(cx);
    }
}

/// `https:`·`mailto:`처럼 스킴이 있는 주소인지(`file:`은 경로로 본다). 한 글자 스킴은 Windows 드라이브
/// (`C:\…`)라 경로다.
fn has_scheme(url: &str) -> bool {
    let Some((scheme, _)) = url.split_once(':') else {
        return false;
    };
    scheme.len() > 1
        && !scheme.eq_ignore_ascii_case("file")
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// `%XX`를 바이트로 푼다(`%20` → 공백, UTF-8 한글 등). 잘못된 `%`는 그대로 둔다.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut ix = 0;
    while ix < bytes.len() {
        let hex = |byte: u8| (byte as char).to_digit(16);
        if bytes[ix] == b'%'
            && let (Some(high), Some(low)) = (
                bytes.get(ix + 1).copied().and_then(hex),
                bytes.get(ix + 2).copied().and_then(hex),
            )
        {
            out.push((high * 16 + low) as u8);
            ix += 3;
        } else {
            out.push(bytes[ix]);
            ix += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    // `gpui::*`의 `test` 매크로가 표준 `#[test]`를 가리지 않도록 glob으로 가져오지 않는다.
    use super::{has_scheme, percent_decode};

    #[test]
    fn schemes_and_paths() {
        for url in ["https://a.b", "http://a", "mailto:a@b.c", "obsidian://open"] {
            assert!(has_scheme(url), "{url}");
        }
        for url in [
            "../a.md",
            "a.md",
            "/Users/a.md",
            "C:\\a.md",
            "file:///a.md",
            "#제목",
        ] {
            assert!(!has_scheme(url), "{url}");
        }
    }

    #[test]
    fn percent_decoding() {
        assert_eq!(percent_decode("a%20b.md"), "a b.md");
        assert_eq!(percent_decode("%EB%A9%94%EB%AA%A8.md"), "메모.md");
        assert_eq!(percent_decode("100%.md"), "100%.md");
    }
}
