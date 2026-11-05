//! `.hanji` 프로젝트 파일 가져오기: 파일을 골라 라이브러리의 `Projects/`에 풀고 프로젝트 화면으로 연다.

use gpui_kit::gpui::*;

use super::HanjiApp;
use crate::library;

impl HanjiApp {
    pub(super) fn on_import_project(
        &mut self,
        _: &crate::ImportProject,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.import_project(window, cx);
    }

    /// OS 파일 선택기로 `.hanji`(또는 예전 `.writer`) 파일을 골라 새 프로젝트로 가져온다.
    pub(super) fn import_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.library.as_ref().map(|lib| lib.root.clone()) else {
            self.set_notice(t!("library.project.import_no_library"), cx);
            return;
        };
        let projects = root.join(library::SectionKind::Projects.dir());
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t!("library.project.import_pick").into()),
        });

        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = receiver.await else {
                return;
            };
            let Some(archive) = paths.pop() else {
                return;
            };
            let result = cx
                .background_executor()
                .spawn(async move { crate::export::project::import_from_file(&archive, &projects) })
                .await;

            this.update_in(cx, |app, window, cx| match result {
                Ok(dir) => {
                    let name = dir
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    match library::scan(&root) {
                        Ok(scanned) => {
                            app.library = Some(scanned);
                            app.enter_project(&dir, window, cx);
                        }
                        Err(err) => {
                            app.set_notice(t!("library.notice.read_failed", err = err), cx);
                            return;
                        }
                    }
                    app.set_notice(t!("library.project.import_done", name = name), cx);
                }
                Err(err) => app.set_notice(t!("library.project.import_failed", err = err), cx),
            })
            .ok();
        })
        .detach();
    }
}
