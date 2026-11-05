//! 자동 업데이트 화면: 시작할 때와 `업데이트 확인…` 메뉴에서 새 버전을 찾고, 물어본 뒤 내려받아 설치한다.
//! 확인·내려받기·서명 확인·앱 교체는 `hanji-update` 크레이트가 맡는다.

use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::{Dialog, DialogFooter};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, WindowExt as _};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::*;
use hanji_update::{Installation, Release, Version};

use super::HanjiApp;
use super::style::Palette;

/// 시작하고 이만큼 기다렸다가 확인한다(첫 화면을 그리는 데 방해되지 않게).
const STARTUP_DELAY: Duration = Duration::from_secs(5);

/// 대화상자 폭.
const DIALOG_WIDTH: f32 = 440.0;

/// 업데이트 진행 상태.
#[derive(Default)]
pub(super) enum UpdateState {
    #[default]
    Idle,
    /// 확인하거나 내려받는 중.
    Busy,
    /// 내려받아(macOS는 설치까지) 다시 시작만 남음.
    Ready {
        version: Version,
        installation: Installation,
        downloaded: std::path::PathBuf,
    },
}

/// 업데이트 알림의 id. 확인 중 → 결과처럼 이어지는 알림이 쌓이지 않고 앞의 것을 바꾼다.
struct UpdateNotice;

/// 업데이트 대화상자가 떠 있다는 표시. 닫기 단추는 대화상자의 `on_close`를 부르지 않아 직접 지운다.
struct UpdateDialogOpen;

impl Global for UpdateDialogOpen {}

/// 업데이트 대화상자를 닫는다.
fn close_update_dialog(window: &mut Window, cx: &mut App) {
    window.close_dialog(cx);
    if cx.has_global::<UpdateDialogOpen>() {
        cx.remove_global::<UpdateDialogOpen>();
    }
}

fn current_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Version(0, 0, 0))
}

impl HanjiApp {
    /// 시작할 때 한 번, 조용히 확인한다. 개발 빌드나 스스로 바꿀 수 없는 설치(번들 밖, ad-hoc 서명)는
    /// 건너뛴다. 설정 `check_updates = false`면 하지 않는다.
    pub(super) fn schedule_update_check(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if cfg!(debug_assertions)
            || !crate::config::Config::load().check_updates()
            || hanji_update::installation().is_err()
        {
            return;
        }
        self.update_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(STARTUP_DELAY).await;
            this.update_in(cx, |this, window, cx| {
                this.check_for_updates(false, window, cx)
            })
            .ok();
        }));
    }

    fn notify_update(
        &mut self,
        message: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.push_notification(Notification::info(message.into()).id::<UpdateNotice>(), cx);
        cx.notify();
    }

    /// 업데이트 대화상자를 연다. 앞서 띄운 업데이트 대화상자가 아직 떠 있으면 닫고 연다(겹치지 않게).
    fn open_update_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        build: impl Fn(Dialog, &mut Window, &mut App) -> Dialog + 'static,
    ) {
        if cx.try_global::<UpdateDialogOpen>().is_some() && window.has_active_dialog(cx) {
            window.close_dialog(cx);
        }
        cx.set_global(UpdateDialogOpen);
        window.open_dialog(cx, move |dialog, window, cx| {
            build(dialog, window, cx).on_close(|_, _, cx| {
                if cx.has_global::<UpdateDialogOpen>() {
                    cx.remove_global::<UpdateDialogOpen>();
                }
            })
        });
    }

    pub(super) fn on_check_for_updates(
        &mut self,
        _: &crate::CheckForUpdates,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.check_for_updates(true, window, cx);
    }

    /// 새 버전을 찾는다. `manual`이면 결과(최신임·오류)도 알린다.
    fn check_for_updates(&mut self, manual: bool, window: &mut Window, cx: &mut Context<Self>) {
        match &self.update {
            UpdateState::Busy => {
                if manual {
                    self.notify_update(t!("update.busy"), window, cx);
                }
                return;
            }
            UpdateState::Ready { version, .. } => {
                let version = *version;
                self.open_ready_dialog(version, window, cx);
                return;
            }
            UpdateState::Idle => {}
        }
        self.update = UpdateState::Busy;
        if manual {
            self.notify_update(t!("update.checking"), window, cx);
        }
        let current = current_version();
        self.update_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { hanji_update::check(current) })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.update = UpdateState::Idle;
                match result {
                    Ok(Some(release)) => this.open_available_dialog(release, window, cx),
                    Ok(None) if manual => this.notify_update(
                        t!("update.up_to_date", version = current.to_string()),
                        window,
                        cx,
                    ),
                    Err(err) if manual => {
                        this.notify_update(t!("update.failed", err = err.to_string()), window, cx)
                    }
                    _ => {}
                }
            })
            .ok();
        }));
    }

    /// 새 버전 안내: 버전, 릴리스 노트, [나중에] [업데이트]. 스스로 바꿀 수 없는 설치면 이유를 보이고
    /// 다운로드 페이지를 연다.
    fn open_available_dialog(
        &mut self,
        release: Release,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let installation = hanji_update::installation();
        let app = cx.entity().downgrade();
        let current = current_version();
        self.open_update_dialog(window, cx, move |dialog, _, cx| {
            let pal = Palette::from_theme(cx.theme());
            let notes = release.notes.trim().to_string();
            let unsupported = installation.as_ref().err().map(ToString::to_string);
            let body = div()
                .flex()
                .flex_col()
                .gap_3()
                .text_sm()
                .text_color(pal.foreground)
                .child(
                    t!(
                        "update.available",
                        version = release.version.to_string(),
                        current = current.to_string()
                    )
                    .to_string(),
                )
                .when(!notes.is_empty(), |this| {
                    this.child(
                        div()
                            .id("update-notes")
                            .max_h(px(220.))
                            .overflow_y_scroll()
                            .p_2()
                            .rounded_md()
                            .border_1()
                            .border_color(pal.border)
                            .text_xs()
                            .text_color(pal.muted)
                            .child(notes),
                    )
                })
                .when_some(unsupported.clone(), |this, reason| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(pal.muted)
                            .child(t!("update.unsupported", reason = reason).to_string()),
                    )
                });
            let primary = match &installation {
                Ok(installation) => {
                    let app = app.clone();
                    let release = release.clone();
                    let installation = installation.clone();
                    Button::new("update-install")
                        .primary()
                        .label(t!("update.install"))
                        .on_click(move |_, window, cx| {
                            close_update_dialog(window, cx);
                            app.update(cx, |app, cx| {
                                app.download_update(
                                    release.clone(),
                                    installation.clone(),
                                    window,
                                    cx,
                                )
                            })
                            .ok();
                        })
                }
                Err(_) => {
                    let url = release.page_url.clone();
                    Button::new("update-open-page")
                        .primary()
                        .label(t!("update.open_page"))
                        .on_click(move |_, window, cx| {
                            close_update_dialog(window, cx);
                            cx.open_url(&url);
                        })
                }
            };
            dialog
                .w(px(DIALOG_WIDTH))
                .title(t!("update.title"))
                .close_button(false)
                .child(body)
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("update-later")
                                .label(t!("update.later"))
                                .on_click(|_, window, cx| close_update_dialog(window, cx)),
                        )
                        .child(primary),
                )
        });
    }

    /// 내려받고, macOS는 서명을 확인해 앱을 바꾼 뒤 다시 시작할지 묻는다.
    fn download_update(
        &mut self,
        release: Release,
        installation: Installation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(self.update, UpdateState::Idle) {
            return;
        }
        self.update = UpdateState::Busy;
        self.notify_update(
            t!("update.downloading", version = release.version.to_string()),
            window,
            cx,
        );
        let version = release.version;
        self.update_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let progress = hanji_update::Progress::default();
                    let downloaded = hanji_update::download(&release.asset, &progress)?;
                    // macOS는 지금 바꿔 둔다(실행 중인 앱은 그대로 돈다). Windows는 앱이 끝나야 설치할 수 있다.
                    if cfg!(target_os = "macos") {
                        hanji_update::install(&installation, &downloaded)?;
                    }
                    Ok::<_, hanji_update::Error>((installation, downloaded))
                })
                .await;
            this.update_in(cx, |this, window, cx| match result {
                Ok((installation, downloaded)) => {
                    this.update = UpdateState::Ready {
                        version,
                        installation,
                        downloaded,
                    };
                    this.open_ready_dialog(version, window, cx);
                }
                Err(err) => {
                    this.update = UpdateState::Idle;
                    this.notify_update(t!("update.failed", err = err.to_string()), window, cx);
                }
            })
            .ok();
        }));
    }

    /// 준비됨: [나중에] [다시 시작].
    fn open_ready_dialog(&mut self, version: Version, window: &mut Window, cx: &mut Context<Self>) {
        let app = cx.entity().downgrade();
        let message = if cfg!(target_os = "macos") {
            t!("update.ready_installed", version = version.to_string())
        } else {
            t!("update.ready_downloaded", version = version.to_string())
        };
        self.open_update_dialog(window, cx, move |dialog, _, cx| {
            let pal = Palette::from_theme(cx.theme());
            let app = app.clone();
            dialog
                .w(px(DIALOG_WIDTH))
                .title(t!("update.ready_title"))
                .close_button(false)
                .child(
                    div()
                        .text_sm()
                        .text_color(pal.foreground)
                        .child(message.to_string()),
                )
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("update-ready-later")
                                .label(t!("update.later"))
                                .on_click(|_, window, cx| close_update_dialog(window, cx)),
                        )
                        .child(
                            Button::new("update-restart")
                                .primary()
                                .label(t!("update.restart"))
                                .on_click(move |_, window, cx| {
                                    close_update_dialog(window, cx);
                                    app.update(cx, |app, cx| app.restart_to_update(window, cx))
                                        .ok();
                                }),
                        ),
                )
        });
    }

    /// 새 버전으로 다시 시작한다. 열린 시트는 저장하고 닫는다.
    fn restart_to_update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let UpdateState::Ready {
            installation,
            downloaded,
            ..
        } = &self.update
        else {
            return;
        };
        let result = if cfg!(target_os = "macos") {
            hanji_update::relaunch(installation)
        } else {
            hanji_update::install(installation, downloaded)
        };
        if let Err(err) = result {
            self.notify_update(t!("update.failed", err = err.to_string()), window, cx);
            return;
        }
        self.close_sheet(cx);
        cx.quit();
    }
}
