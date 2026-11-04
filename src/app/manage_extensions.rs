//! 확장 관리 대화상자(ADR-0026): 라이브러리의 확장을 모두 보이고 휴지통으로 옮긴다.
//!
//! 그림을 그리는 확장과 명령만 있는 확장, 읽지 못하는 확장을 함께 보인다. 왼쪽 목록에서 고르면 오른쪽에
//! 미리보기(앞머리의 `example` 인자로 그린다, 삽입 대화상자와 같은 그리기)나 명령 목록, 설명, 그 확장을
//! 쓰는 시트 수를 보인다. 시트 수는 대화상자를 열 때 백그라운드에서 한 번 센다.
//! 지운 뒤 메뉴·에디터 블록은 폴더 감시가 다시 스캔하며 맞춘다.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogButtonProps, DialogFooter};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, WindowExt as _};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::*;

use super::HanjiApp;
use super::editor_extension::{Player, Ticker};
use super::insert_extension::render_preview;
use super::style::Palette;
use crate::extension::{self, Host, Info, Meta};
use crate::{ManageExtensions, icons, library};

/// 대화상자 폭.
const DIALOG_WIDTH: f32 = 720.0;

/// 목록 칸 폭.
const LIST_WIDTH: f32 = 220.0;

/// 미리보기 칸 크기.
const PREVIEW_WIDTH: f32 = 440.0;
const PREVIEW_HEIGHT: f32 = 220.0;

/// 확장 하나: 앞머리와 앱이 읽은 정보(읽지 못하면 그 까닭).
struct Item {
    meta: Meta,
    info: Result<Info, String>,
}

impl Item {
    /// 종류: 그림·애니메이션, 명령 수.
    fn kind(&self) -> String {
        let Ok(info) = &self.info else {
            return t!("extensions.unreadable").to_string();
        };
        let mut parts = Vec::new();
        if info.draws {
            parts.push(
                if info.animated {
                    t!("extensions.kind_animation")
                } else {
                    t!("extensions.kind_drawing")
                }
                .to_string(),
            );
        }
        if !info.commands.is_empty() {
            parts.push(t!("extensions.kind_commands", count = info.commands.len()).to_string());
        }
        parts.join(" · ")
    }
}

/// 확장 목록, 고른 것, 사용처 수.
struct ExtensionManager {
    root: PathBuf,
    app: WeakEntity<HanjiApp>,
    host: Host,
    items: Vec<Item>,
    selected: usize,
    /// 미리보기 캔버스. 확장을 고를 때마다 처음부터 다시 그린다.
    player: std::rc::Rc<Player>,
    ticker: Ticker,
    /// 확장 이름 → 쓰는 시트 수. 세는 동안은 `None`.
    usage: Option<HashMap<String, usize>>,
    _count: Task<()>,
}

impl ExtensionManager {
    fn new(
        root: &Path,
        sheets: Vec<PathBuf>,
        app: WeakEntity<HanjiApp>,
        cx: &mut Context<Self>,
    ) -> Self {
        let count = cx.spawn(async move |this, cx| {
            let usage = cx
                .background_executor()
                .spawn(async move { count_usage(&sheets) })
                .await;
            this.update(cx, |this, cx| {
                this.usage = Some(usage);
                cx.notify();
            })
            .ok();
        });
        let mut manager = Self {
            root: root.to_path_buf(),
            app,
            host: Host::new(root),
            items: Vec::new(),
            selected: 0,
            player: std::rc::Rc::new(Player::new()),
            ticker: Ticker::default(),
            usage: None,
            _count: count,
        };
        manager.reload();
        manager
    }

    /// 확장 폴더를 다시 읽는다.
    fn reload(&mut self) {
        self.items = extension::catalog(&self.root)
            .into_iter()
            .map(|meta| {
                let info = match &*self.host.get(&meta.name) {
                    Ok(worker) => Ok(worker.info.clone()),
                    Err(err) => Err(err.clone()),
                };
                Item { meta, info }
            })
            .collect();
    }

    fn select(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.selected = ix;
        self.player = std::rc::Rc::new(Player::new());
        cx.notify();
    }

    /// 확인을 받고 확장 `name`을 휴지통으로 옮긴다.
    fn confirm_delete(&self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let used = self
            .usage
            .as_ref()
            .and_then(|usage| usage.get(name))
            .copied()
            .unwrap_or(0);
        let mut description = t!("extensions.delete_description", name = name).to_string();
        if used > 0 {
            description.push_str(&t!("extensions.delete_used", used = used));
        }
        let manager = cx.entity().downgrade();
        let name = name.to_string();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let manager = manager.clone();
            let name = name.clone();
            alert
                .confirm()
                .title(t!("extensions.delete_title"))
                .description(description.clone())
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t!("common.delete"))
                        .ok_variant(ButtonVariant::Danger)
                        .cancel_text(t!("common.cancel")),
                )
                .on_ok(move |_, window, cx| {
                    manager
                        .update(cx, |manager, cx| manager.delete(&name, window, cx))
                        .ok();
                    true
                })
        });
    }

    fn delete(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let result = library::move_to_trash(&extension::location(&self.root, name));
        self.reload();
        self.select(self.selected.min(self.items.len().saturating_sub(1)), cx);
        let app = self.app.clone();
        app.update(cx, |app, cx| match result {
            Ok(()) => app.refresh_extension_commands(cx),
            Err(err) => app.notify(t!("extensions.delete_failed", err = err), window, cx),
        })
        .ok();
    }
}

/// 시트마다 쓰는 확장 이름을 모아, 확장마다 그 확장을 쓰는 시트 수를 센다.
fn count_usage(sheets: &[PathBuf]) -> HashMap<String, usize> {
    let mut usage = HashMap::new();
    for sheet in sheets {
        let Ok(text) = std::fs::read_to_string(sheet) else {
            continue;
        };
        let mut names: Vec<String> = extension::blocks(&text)
            .into_iter()
            .map(|block| block.name)
            .collect();
        names.sort();
        names.dedup();
        for name in names {
            *usage.entry(name).or_insert(0) += 1;
        }
    }
    usage
}

impl Render for ExtensionManager {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = Palette::from_theme(cx.theme());
        let Some(item) = self.items.get(self.selected) else {
            return div()
                .h(px(PREVIEW_HEIGHT))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_1()
                .text_sm()
                .text_color(pal.muted)
                .child(t!("extensions.none"))
                .child(t!("extensions.ask_assistant"))
                .into_any_element();
        };
        let list = div()
            .id("extension-manager-list")
            .w(px(LIST_WIDTH))
            .h(px(PREVIEW_HEIGHT + 120.))
            .flex_none()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_0p5()
            .children(self.items.iter().enumerate().map(|(ix, item)| {
                let selected = ix == self.selected;
                div()
                    .id(("extension-manager-item", ix))
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .when(selected, |this| this.bg(pal.active))
                    .when(!selected, |this| this.hover(|this| this.bg(pal.hover)))
                    .on_click(cx.listener(move |manager, _, _, cx| {
                        if manager.selected != ix {
                            manager.select(ix, cx);
                        }
                    }))
                    .child(div().text_sm().child(item.meta.name.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(if item.info.is_err() {
                                pal.danger
                            } else {
                                pal.muted
                            })
                            .child(item.kind()),
                    )
            }));

        let name = item.meta.name.clone();
        let boxed = |content: AnyElement| {
            div()
                .h(px(PREVIEW_HEIGHT))
                .rounded_md()
                .border_1()
                .border_color(pal.border)
                .bg(pal.background)
                .overflow_hidden()
                .child(content)
        };
        let centered = |color: Hsla, text: String| {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .p_4()
                .text_xs()
                .text_color(color)
                .child(text)
                .into_any_element()
        };
        // 그리는 확장은 예시 인자로 그린다. 명령만 있으면 명령 제목을, 읽지 못하면 까닭을 보인다.
        let preview = match &item.info {
            Err(err) => centered(pal.danger, err.clone()),
            Ok(info) if info.draws => render_preview(
                &self.host,
                &self.player,
                &self.ticker,
                &name,
                &item.meta.example,
                (PREVIEW_WIDTH, PREVIEW_HEIGHT),
                None,
                pal,
                window,
                cx,
            ),
            Ok(info) => div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_1()
                .text_xs()
                .child(
                    div()
                        .text_color(pal.muted)
                        .child(t!("extensions.menu_commands")),
                )
                .children(info.commands.iter().map(|title| div().child(title.clone())))
                .into_any_element(),
        };
        let usage = match &self.usage {
            None => t!("extensions.counting_usage").to_string(),
            Some(usage) => match usage.get(&name) {
                Some(count) => t!("extensions.used_in", count = count).to_string(),
                None => t!("extensions.unused").to_string(),
            },
        };
        let file = extension::location(&self.root, &name);
        // 머리줄: 이름과 종류·사용처, 오른쪽 끝에 기능 버튼(아이콘, 툴팁).
        let subtitle = match item.kind() {
            kind if kind.is_empty() => usage,
            kind => format!("{kind} · {usage}"),
        };
        let header = div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_baseline()
                    .gap_2()
                    .child(
                        div()
                            .flex_none()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(name.clone()),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_xs()
                            .text_color(pal.muted)
                            .child(subtitle),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .gap_1()
                    .child(
                        Button::new("extension-reveal")
                            .ghost()
                            .small()
                            .icon(icons::reveal())
                            .tooltip(t!("common.reveal_in_finder"))
                            .on_click(move |_, _, cx| cx.reveal_path(&file)),
                    )
                    .child(
                        Button::new("extension-delete")
                            .ghost()
                            .small()
                            .icon(Icon::new(icons::trash()).text_color(pal.danger))
                            .tooltip(t!("common.move_to_trash"))
                            .on_click(cx.listener(move |manager, _, window, cx| {
                                manager.confirm_delete(&name, window, cx);
                            })),
                    ),
            );
        let details = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_2()
            .child(header)
            .child(boxed(preview))
            .child(
                div()
                    .text_sm()
                    .when(item.meta.description.is_empty(), |this| {
                        this.text_color(pal.muted)
                            .child(t!("extensions.no_description"))
                    })
                    .child(item.meta.description.clone()),
            );
        div()
            .flex()
            .gap_3()
            .child(list)
            .child(details)
            .into_any_element()
    }
}

impl HanjiApp {
    /// 확장 관리 대화상자를 연다.
    pub(super) fn on_manage_extensions(
        &mut self,
        _: &ManageExtensions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(library) = &self.library else {
            self.notify(t!("common.open_library_first"), window, cx);
            return;
        };
        let root = library.root.clone();
        let sheets = library
            .sections
            .iter()
            .flat_map(|section| section.sheets())
            .map(|sheet| sheet.path.clone())
            .collect();
        let app = cx.entity().downgrade();
        let manager = cx.new(|cx| ExtensionManager::new(&root, sheets, app, cx));
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .w(px(DIALOG_WIDTH))
                .title(t!("extensions.manage_title"))
                .close_button(false)
                .child(manager.clone())
                .footer(
                    DialogFooter::new().child(
                        Button::new("manage-extensions-close")
                            .label(t!("common.close"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    ),
                )
        });
    }
}
