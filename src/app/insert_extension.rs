//! 확장 삽입·편집 대화상자(ADR-0026, ADR-0033): 그림을 그리는 확장을 미리보기와 설명을 보고 골라 커서 자리에
//! `` ```ext `` 블록으로 넣거나, 시트에 있는 블록 하나의 인자를 고친다.
//!
//! 목록·설명·인자 설명은 확장 앞머리(`extension::Meta`)에서 읽는다. 인자마다 입력 칸이 있고 처음 값은
//! 넣을 때는 앞머리의 `example`, 고칠 때는 블록의 인자다. 미리보기는 입력한 값으로 실제 `draw`를 불러
//! 그리고(에디터 블록과 같은 그리기), 값을 고치면 바로 다시 그린다. 영역(`g.region`)을 누르면 `click`이
//! 돌려준 값을 칸에 쓴다. 넣는 블록의 인자도 입력한 값이다(빈 칸은 빼서 확장의 기본값을 쓴다).
//! 고칠 때는 블록 원문을 직접 고칠 수도 있다.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::input::{
    EditorState, Input, InputEvent, InputState, Textarea, TextareaState,
};
use gpui_kit::component::{ActiveTheme as _, FocusableExt as _, Sizable as _, WindowExt as _};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::*;

use super::HanjiApp;
use super::editor_extension::{
    Frame, OnChanges, Player, Ticker, block_range, fit, interactive, scaled, shapes_canvas, theme,
};
use super::style::Palette;
use crate::InsertExtension;
use crate::extension::{self, Host, Meta};
use std::ops::Range;
use std::rc::Rc;

/// 대화상자 폭.
const DIALOG_WIDTH: f32 = 720.0;

/// 목록 칸 폭.
const LIST_WIDTH: f32 = 220.0;

/// 미리보기 칸 크기.
const PREVIEW_WIDTH: f32 = 440.0;
const PREVIEW_HEIGHT: f32 = 220.0;

/// 인자 입력 칸 폭.
const ARG_INPUT_WIDTH: f32 = 150.0;

/// 고른 확장의 인자 하나: 이름, 앞머리의 설명, 입력 칸.
struct ArgField {
    key: String,
    description: String,
    input: Entity<InputState>,
}

/// 시트에 있는 블록 하나를 고치는 중: 그 블록이 있던 에디터와 범위(펜스 포함, 바이트), 그때의 원문.
#[derive(Clone)]
struct EditBlock {
    state: Entity<EditorState>,
    range: Range<usize>,
    original: String,
    /// 문단 안의 인라인 확장(`{{…}}`, ADR-0036)이면 `true`. 자리는 없고, 배율은 줄 높이에 대한 것이다.
    inline: bool,
}

/// 그림을 그리는 확장 목록과 고른 것.
struct ExtensionPicker {
    host: Host,
    items: Vec<Meta>,
    selected: usize,
    /// 미리보기 캔버스. 확장을 고를 때마다 처음부터 다시 그린다.
    player: Rc<Player>,
    ticker: Ticker,
    /// 고른 확장의 인자 칸. 앞머리의 `args` 순서, 그다음 처음 값에만 있는 인자, 마지막에 `scale`.
    fields: Vec<ArgField>,
    /// 칸의 처음 값(되돌리기 기준): 넣을 때는 `example`, 고칠 때는 블록의 인자.
    baseline: Vec<(String, String)>,
    /// 고치는 블록. 있으면 확장 목록을 숨기고 그 확장 하나만 다룬다.
    edit: Option<EditBlock>,
    /// 원문을 직접 고치는 중이면 그 입력창. 미리보기와 저장이 이 글을 쓴다.
    source: Option<Entity<TextareaState>>,
    /// 블록 자리(`align:`)와 내보내기(`export:`). `None`은 적지 않는다(가운데, 확장의 기본값).
    align: Option<extension::Align>,
    export: Option<bool>,
    _subscriptions: Vec<Subscription>,
}

impl ExtensionPicker {
    /// 넣기: 그림을 그리는 확장을 모두 보인다.
    fn new(root: &std::path::Path, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let host = Host::new(root);
        // 블록으로 넣을 수 있는 것(`draw`가 있는 것)만 보인다. 읽지 못하는 확장도 보여 까닭을 알린다.
        let items = extension::catalog(root)
            .into_iter()
            .filter(|meta| {
                host.get(&meta.name)
                    .as_ref()
                    .as_ref()
                    .map_or(true, |worker| worker.info.draws)
            })
            .collect();
        Self::with_items(host, items, None, None, window, cx)
    }

    /// 고치기: 블록의 확장 `name` 하나를 블록 인자 `args`로 연다. 확장 파일이 없어도 열어 원문을 고칠 수 있다.
    fn edit(
        root: &std::path::Path,
        name: &str,
        args: Vec<(String, String)>,
        edit: EditBlock,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let host = Host::new(root);
        let meta = extension::catalog(root)
            .into_iter()
            .find(|meta| meta.name == name)
            .unwrap_or_else(|| Meta {
                name: name.to_string(),
                ..Meta::default()
            });
        Self::with_items(host, vec![meta], Some(args), Some(edit), window, cx)
    }

    fn with_items(
        host: Host,
        items: Vec<Meta>,
        initial: Option<Vec<(String, String)>>,
        edit: Option<EditBlock>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut picker = Self {
            host,
            items,
            selected: 0,
            player: Rc::new(Player::new()),
            ticker: Ticker::default(),
            fields: Vec::new(),
            baseline: Vec::new(),
            edit,
            source: None,
            align: None,
            export: None,
            _subscriptions: Vec::new(),
        };
        picker.select(0, initial, window, cx);
        picker
    }

    fn selected(&self) -> Option<&Meta> {
        self.items.get(self.selected)
    }

    /// 확장 `ix`를 고르고 인자 칸을 `initial`(없으면 `example`) 값으로 채운다.
    fn select(
        &mut self,
        ix: usize,
        initial: Option<Vec<(String, String)>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected = ix;
        self.player = Rc::new(Player::new());
        self.fields.clear();
        self._subscriptions.clear();
        self.source = None;
        let Some(meta) = self.items.get(ix).cloned() else {
            return;
        };
        self.baseline = initial.unwrap_or_else(|| meta.example.clone());
        let baseline = self.baseline.clone();
        self.align = baseline
            .iter()
            .any(|(key, _)| key == extension::ALIGN_ARG)
            .then(|| extension::align_of(&baseline));
        self.export = extension::export_arg(&baseline);
        // 같은 인자가 여럿이면 마지막 것이 쓰인다(블록을 읽는 규칙과 같다).
        let initial = |key: &str| {
            baseline
                .iter()
                .rev()
                .find(|(arg, _)| arg == key)
                .map(|(_, value)| value.clone())
                .unwrap_or_default()
        };
        let mut extra: Vec<(String, String)> = Vec::new();
        for (key, _) in &baseline {
            if !extension::RESERVED_ARGS.contains(&key.as_str())
                && !meta.args.iter().any(|(arg, _)| arg == key)
                && !extra.iter().any(|(seen, _)| seen == key)
            {
                extra.push((key.clone(), String::new()));
            }
        }
        // 배율은 앱이 처리하므로 어느 확장에나 있다(인라인 확장은 줄 높이에 대한 배율).
        let scale = (
            extension::SCALE_ARG.to_string(),
            t!("extensions.scale_arg").to_string(),
        );
        let fields = meta
            .args
            .iter()
            .filter(|(key, _)| !extension::RESERVED_ARGS.contains(&key.as_str()))
            .cloned()
            .chain(extra)
            .chain(std::iter::once(scale));
        for (key, description) in fields {
            let value = initial(&key);
            self.add_field(key, description, value, window, cx);
        }
        cx.notify();
    }

    fn add_field(
        &mut self,
        key: String,
        description: String,
        value: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("extensions.arg_default"))
                .default_value(value)
        });
        self._subscriptions
            .push(cx.subscribe(&input, |_, _, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    cx.notify();
                }
            }));
        // `scale`은 늘 마지막 칸이다.
        let at = self
            .fields
            .iter()
            .position(|field| field.key == extension::SCALE_ARG)
            .unwrap_or(self.fields.len());
        self.fields.insert(
            at,
            ArgField {
                key,
                description,
                input,
            },
        );
    }

    /// 입력한 인자 값. 빈 칸은 뺀다(확장이 기본값을 쓴다). 원문을 고치는 중이면 그 원문의 인자다.
    fn args(&self, cx: &App) -> Vec<(String, String)> {
        if let Some(source) = &self.source {
            let text = source.read(cx).value();
            if self.is_inline() {
                return extension::inline_calls(&text)
                    .into_iter()
                    .next()
                    .map(|call| call.args)
                    .unwrap_or_default();
            }
            return extension::blocks(&text)
                .into_iter()
                .next()
                .map(|block| block.args)
                .unwrap_or_default();
        }
        let mut args: Vec<(String, String)> = self
            .fields
            .iter()
            .filter_map(|field| {
                let value = field.input.read(cx).value().trim().to_string();
                (!value.is_empty()).then(|| (field.key.clone(), value))
            })
            .collect();
        if let Some(align) = self.align {
            args.push((
                extension::ALIGN_ARG.to_string(),
                align_name(align).to_string(),
            ));
        }
        if let Some(export) = self.export {
            args.push((extension::EXPORT_ARG.to_string(), export.to_string()));
        }
        args
    }

    /// 저장할 블록 글. 원문을 고치는 중이면 그 글 그대로다.
    fn block_text(&self, cx: &App) -> Option<String> {
        if let Some(source) = &self.source {
            return Some(source.read(cx).value().trim_end_matches('\n').to_string());
        }
        let meta = self.selected()?;
        Some(if self.is_inline() {
            extension::inline_text(&meta.name, &self.args(cx))
        } else {
            extension::block_text(&meta.name, &self.args(cx))
        })
    }

    /// 고른 확장이 내보내기에 넣는 것이 기본인지. 확장을 읽지 못하면 넣는다(그리지 못하면 코드 블록이다).
    fn export_default(&self) -> bool {
        self.selected().is_none_or(|meta| {
            self.host
                .get(&meta.name)
                .as_ref()
                .as_ref()
                .map_or(true, |worker| worker.info.export)
        })
    }

    fn is_inline(&self) -> bool {
        self.edit.as_ref().is_some_and(|edit| edit.inline)
    }

    /// 누르기(`click`)가 돌려준 값을 칸에 쓴다. 칸이 없는 인자는 칸을 더한다.
    fn apply_changes(
        &mut self,
        changes: &[(String, String)],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.source.is_some() {
            return;
        }
        for (key, value) in changes {
            if key == extension::ALIGN_ARG {
                self.align = Some(extension::align_of(&[(key.clone(), value.clone())]));
                continue;
            }
            if key == extension::EXPORT_ARG {
                self.export = extension::export_arg(&[(key.clone(), value.clone())]);
                continue;
            }
            match self.fields.iter().find(|field| field.key == *key) {
                Some(field) => field
                    .input
                    .update(cx, |input, cx| input.set_value(value.clone(), window, cx)),
                None => self.add_field(key.clone(), String::new(), value.clone(), window, cx),
            }
        }
        cx.notify();
    }

    /// 인자 칸을 처음 값으로 되돌린다.
    fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.select(self.selected, Some(self.baseline.clone()), window, cx);
    }

    /// 원문 고치기를 켜고 끈다. 켤 때는 지금 인자로 만든 블록 글(고치지 않았으면 블록 원문)로 시작한다.
    fn toggle_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.source.take().is_some() {
            cx.notify();
            return;
        }
        let unchanged = sorted(self.args(cx)) == sorted(self.baseline.clone());
        let text = match &self.edit {
            Some(edit) if unchanged => edit.original.clone(),
            _ => self.block_text(cx).unwrap_or_default(),
        };
        let rows = if self.is_inline() { (2, 4) } else { (6, 14) };
        let source = cx.new(|cx| TextareaState::new(window, cx).auto_grow(rows.0, rows.1));
        source.update(cx, |source, cx| source.set_value(text, window, cx));
        self._subscriptions
            .push(cx.subscribe(&source, |_, _, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    cx.notify();
                }
            }));
        self.source = Some(source);
        cx.notify();
    }
}

fn align_name(align: extension::Align) -> &'static str {
    match align {
        extension::Align::Left => "left",
        extension::Align::Center => "center",
        extension::Align::Right => "right",
    }
}

/// 순서와 상관없이 비교하려고 정렬한 인자.
fn sorted(mut pairs: Vec<(String, String)>) -> Vec<(String, String)> {
    pairs.sort();
    pairs
}

/// 확장 `name`을 `args`로 그린 미리보기. `width`×`height` 칸 가운데에 비율을 지켜 맞춘다.
/// 읽기·그리기 오류는 칸에 글로 보인다. 확장 관리 대화상자도 쓴다. `on_changes`가 있으면 영역을 누를 수
/// 있고, `click`이 돌려준 인자 변경을 그리로 넘긴다.
#[allow(clippy::too_many_arguments)]
pub(super) fn render_preview(
    host: &Host,
    player: &Rc<Player>,
    ticker: &Ticker,
    name: &str,
    args: &[(String, String)],
    (width, height): (f32, f32),
    on_changes: Option<OnChanges>,
    pal: Palette,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let loaded = host.get(name);
    let scale = extension::scale_of(args);
    let args = &extension::without_reserved(args);
    let message = |text: String| {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .p_4()
            .text_xs()
            .text_color(pal.danger)
            .child(text)
            .into_any_element()
    };
    let canvas_size = match &*loaded {
        Ok(worker) => worker.size(args),
        Err(err) => Err(err.clone()),
    };
    let canvas_size = match canvas_size {
        Ok(canvas_size) => canvas_size,
        Err(err) => return message(err),
    };
    let shapes = match player.frame(&loaded, args, theme(pal), ticker, window, cx) {
        Frame::Shapes(shapes) => Some(shapes),
        Frame::Pending => None,
        Frame::Error(err) => return message(err),
    };
    let (fit_width, fit_height) = fit(scaled(canvas_size, scale), px(width));
    // 높이도 칸에 맞춘다(세로로 긴 캔버스).
    let shrink = (height / f32::from(fit_height)).min(1.0);
    let canvas = div()
        .w(fit_width * shrink)
        .h(fit_height * shrink)
        .child(shapes_canvas(
            shapes,
            player.cache(),
            Some(player.bounds()),
            canvas_size,
            fit_width * shrink,
            fit_height * shrink,
        ));
    let canvas = match on_changes {
        Some(on_changes) if player.has_regions() => interactive(
            canvas,
            "extension-preview".into(),
            player.clone(),
            loaded.clone(),
            args.to_vec(),
            canvas_size.0,
            on_changes,
        ),
        _ => canvas.into_any_element(),
    };
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(canvas)
        .into_any_element()
}

impl ExtensionPicker {
    /// 블록 자리(왼쪽·가운데·오른쪽)와 내보내기(확장을 따름·넣기·빼기) 고르기.
    fn render_block_options(&self, pal: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let chip = |id: SharedString, label: std::borrow::Cow<'static, str>, on: bool| {
            div()
                .id(id)
                .px_2()
                .py(px(2.0))
                .rounded_md()
                .cursor_pointer()
                .when(on, |this| this.bg(pal.active).text_color(pal.foreground))
                .when(!on, |this| this.hover(move |style| style.bg(pal.hover)))
                .child(label)
        };
        let align = self.align.unwrap_or(extension::Align::Center);
        let aligns = [
            (extension::Align::Left, t!("extensions.align_left")),
            (extension::Align::Center, t!("extensions.align_center")),
            (extension::Align::Right, t!("extensions.align_right")),
        ]
        .map(|(value, label)| {
            chip(
                format!("extension-align-{}", align_name(value)).into(),
                label,
                align == value,
            )
            .on_click(cx.listener(move |picker, _, _, cx| {
                // 가운데는 기본값이라 적지 않는다.
                picker.align = (value != extension::Align::Center).then_some(value);
                cx.notify();
            }))
        });
        // 내보내기는 이 블록의 실제 결과로 보이고, 확장의 기본값과 다를 때만 블록에 적는다.
        let default = self.export_default();
        let export = Checkbox::new("extension-export")
            .label(t!("extensions.export").to_string())
            .checked(self.export.unwrap_or(default))
            .on_click(cx.listener(move |picker, checked: &bool, _, cx| {
                picker.export = (*checked != default).then_some(*checked);
                cx.notify();
            }));
        let hint = if default {
            t!("extensions.export_hint")
        } else {
            t!("extensions.export_hint_off")
        };
        let inline = self.is_inline();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .text_xs()
            .text_color(pal.muted)
            .when(!inline, |this| {
                this.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_1()
                        .child(div().w(px(72.)).child(t!("extensions.align")))
                        .children(aligns),
                )
            })
            .child(div().flex().flex_col().gap_1().child(export).child(hint))
    }
}

impl Render for ExtensionPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = Palette::from_theme(cx.theme());
        if self.items.is_empty() {
            return div()
                .h(px(PREVIEW_HEIGHT))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_1()
                .text_sm()
                .text_color(pal.muted)
                .child(t!("extensions.no_drawing"))
                .child(t!("extensions.ask_assistant_example"))
                .into_any_element();
        }
        let list = div()
            .id("extension-list")
            .w(px(LIST_WIDTH))
            .h(px(PREVIEW_HEIGHT + 120.))
            .flex_none()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_0p5()
            .children(self.items.iter().enumerate().map(|(ix, meta)| {
                let selected = ix == self.selected;
                div()
                    .id(("extension-item", ix))
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .when(selected, |this| this.bg(pal.active))
                    .when(!selected, |this| this.hover(|this| this.bg(pal.hover)))
                    .on_click(cx.listener(move |picker, _, window, cx| {
                        if picker.selected != ix {
                            picker.select(ix, None, window, cx);
                        }
                    }))
                    .child(div().text_sm().child(meta.name.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(pal.muted)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(meta.description.clone()),
                    )
            }));
        let meta = self.selected().cloned().unwrap_or_default();
        let args = self.args(cx);
        let picker = cx.entity().downgrade();
        let on_changes: OnChanges = Rc::new(move |changes, window, cx| {
            picker
                .update(cx, |picker, cx| {
                    picker.apply_changes(&changes.args, window, cx)
                })
                .ok();
        });
        let value = match &*self.host.get(&meta.name) {
            Ok(worker) if worker.info.values => {
                Some(worker.value(&extension::without_reserved(&args), ""))
            }
            _ => None,
        };
        let preview = match value {
            // 글을 보이는 확장은 그 글을 크게 보인다.
            Some(value) => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .p_4()
                .map(|this| match value {
                    Ok(value) => this.text_xl().child(value.unwrap_or_default()),
                    Err(err) => this.text_xs().text_color(pal.danger).child(err),
                })
                .into_any_element(),
            None => render_preview(
                &self.host,
                &self.player,
                &self.ticker,
                &meta.name,
                &args,
                (PREVIEW_WIDTH, PREVIEW_HEIGHT),
                Some(on_changes),
                pal,
                window,
                cx,
            ),
        };
        // 순서는 상관없이 처음 값과 다른지 본다(칸은 `args` 순서, 처음 값은 적힌 순서다).
        let changed =
            self.source.is_none() && sorted(args.clone()) != sorted(self.baseline.clone());
        let editing = self.edit.is_some();
        let details =
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .h(px(PREVIEW_HEIGHT))
                        .rounded_md()
                        .border_1()
                        .border_color(pal.border)
                        .bg(pal.background)
                        .overflow_hidden()
                        .child(preview),
                )
                .child(
                    div()
                        .text_sm()
                        .when(meta.description.is_empty(), |this| {
                            this.text_color(pal.muted)
                                .child(t!("extensions.no_description"))
                        })
                        .child(meta.description.clone()),
                )
                .when(!self.fields.is_empty() || editing, |this| {
                    let link = |id: &'static str, label: std::borrow::Cow<'static, str>| {
                        div()
                            .id(id)
                            .cursor_pointer()
                            .hover(|this| this.text_color(pal.foreground))
                            .child(label)
                    };
                    this.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .text_xs()
                            .text_color(pal.muted)
                            .child(div().flex_1().child(if self.source.is_some() {
                                t!("extensions.source")
                            } else {
                                t!("extensions.args")
                            }))
                            .when(changed, |this| {
                                this.child(
                                    link(
                                        "extension-args-reset",
                                        if editing {
                                            t!("extensions.reset_block_args")
                                        } else {
                                            t!("extensions.reset_args")
                                        },
                                    )
                                    .on_click(cx.listener(
                                        |picker, _, window, cx| {
                                            picker.reset(window, cx);
                                        },
                                    )),
                                )
                            })
                            .when(editing, |this| {
                                this.child(
                                    link(
                                        "extension-source-toggle",
                                        if self.source.is_some() {
                                            t!("extensions.edit_args")
                                        } else {
                                            t!("extensions.edit_source")
                                        },
                                    )
                                    .on_click(cx.listener(
                                        |picker, _, window, cx| {
                                            picker.toggle_source(window, cx);
                                        },
                                    )),
                                )
                            }),
                    )
                    .children(self.source.as_ref().map(|source| {
                        div()
                            .text_xs()
                            .font_family("monospace")
                            .border_1()
                            .border_color(pal.border)
                            .rounded_md()
                            .p_1()
                            .child(Textarea::new(source).appearance(false))
                    }))
                    .when(self.source.is_none(), |this| {
                        this.child(div().flex().flex_col().gap_1().children(
                            self.fields.iter().map(|field| {
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_xs()
                                    .child(
                                        div()
                                            .w(px(72.))
                                            .flex_none()
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .font_family("monospace")
                                            .child(field.key.clone()),
                                    )
                                    .child(
                                        div().w(px(ARG_INPUT_WIDTH)).flex_none().child(
                                            Input::new(&field.input).small().focus_ring(false),
                                        ),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_color(pal.muted)
                                            .child(field.description.clone()),
                                    )
                            }),
                        ))
                    })
                })
                .when(self.source.is_none() && !self.items.is_empty(), |this| {
                    this.child(self.render_block_options(pal, cx))
                });
        div()
            .flex()
            .gap_3()
            .when(!editing, |this| this.child(list))
            .child(details)
            .into_any_element()
    }
}

impl HanjiApp {
    /// 확장 삽입 대화상자를 연다. 열린 시트가 없으면 안내만 한다.
    pub(super) fn on_insert_extension(
        &mut self,
        _: &InsertExtension,
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
        let target = editor.state().clone();
        let picker = cx.new(|cx| ExtensionPicker::new(&root, window, cx));
        self.open_extension_dialog(picker, target, window, cx);
    }

    /// 줄 `row`가 속한 확장 블록을 편집 대화상자로 연다. 블록이 아니면 알린다.
    pub(super) fn edit_extension_block(
        &mut self,
        row: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.library.as_ref().map(|lib| lib.root.clone()) else {
            return;
        };
        let Some(editor) = &self.editor else {
            return;
        };
        let target = editor.state().clone();
        let text = target.read(cx).value().to_string();
        let found = extension::blocks(&text)
            .into_iter()
            .find(|block| block.rows.contains(&row))
            .and_then(|block| Some((block_range(&text, &block.rows)?, block)));
        let Some((range, block)) = found else {
            self.notify(t!("extensions.no_block_here"), window, cx);
            return;
        };
        let edit = EditBlock {
            state: target.clone(),
            original: text[range.clone()].to_string(),
            range,
            inline: false,
        };
        let picker =
            cx.new(|cx| ExtensionPicker::edit(&root, &block.name, block.args, edit, window, cx));
        self.open_extension_dialog(picker, target, window, cx);
    }

    /// 인라인 확장(`{{…}}`, 시트의 `range`에 있는 `source`)을 편집 대화상자로 연다.
    pub(super) fn edit_inline_extension(
        &mut self,
        range: Range<usize>,
        source: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.library.as_ref().map(|lib| lib.root.clone()) else {
            return;
        };
        let Some(editor) = &self.editor else {
            return;
        };
        let Some(call) = extension::inline_calls(source).into_iter().next() else {
            return;
        };
        let target = editor.state().clone();
        let edit = EditBlock {
            state: target.clone(),
            range,
            original: source.to_string(),
            inline: true,
        };
        let picker =
            cx.new(|cx| ExtensionPicker::edit(&root, &call.name, call.args, edit, window, cx));
        self.open_extension_dialog(picker, target, window, cx);
    }

    /// 넣기·고치기 대화상자를 띄운다. 확인하면 넣거나(고치는 블록이 없을 때) 그 블록을 바꾼다.
    fn open_extension_dialog(
        &mut self,
        picker: Entity<ExtensionPicker>,
        target: Entity<EditorState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let app = cx.entity().downgrade();
        let editing = picker.read(cx).edit.is_some();
        // 고른 확장을 입력한 인자로 넣거나 고친다. 끝났으면 `true`(대화상자를 닫는다).
        let submit = {
            let picker = picker.clone();
            Rc::new(move |window: &mut Window, cx: &mut App| {
                let (name, args, text, edit) = {
                    let picker = picker.read(cx);
                    let Some(meta) = picker.selected() else {
                        return false;
                    };
                    (
                        meta.name.clone(),
                        picker.args(cx),
                        picker.block_text(cx),
                        picker.edit.clone(),
                    )
                };
                app.update(cx, |app, cx| match (edit, text) {
                    (Some(edit), Some(text)) => {
                        app.replace_extension_block(&edit, &text, window, cx)
                    }
                    _ => app.insert_extension(&target, &name, &args, window, cx),
                })
                .ok();
                true
            })
        };
        window.open_dialog(cx, move |dialog, _, cx| {
            let has_items = !picker.read(cx).items.is_empty();
            let (enter, ok) = (submit.clone(), submit.clone());
            dialog
                .w(px(DIALOG_WIDTH))
                .title(if editing {
                    t!("extensions.edit_title")
                } else {
                    t!("extensions.insert_title")
                })
                .close_button(false)
                // 인자 칸의 Enter는 대화상자의 확인으로 올라온다.
                .on_ok(move |_, window, cx| enter(window, cx))
                .child(picker.clone())
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("insert-extension-cancel")
                                .label(t!("common.cancel"))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .when(has_items, |footer| {
                            footer.child(
                                Button::new("insert-extension-ok")
                                    .primary()
                                    .label(if editing {
                                        t!("common.save")
                                    } else {
                                        t!("common.insert")
                                    })
                                    .on_click(move |_, window, cx| {
                                        if ok(window, cx) {
                                            window.close_dialog(cx);
                                        }
                                    }),
                            )
                        }),
                )
        });
    }

    /// 고친 블록을 시트에 쓴다. 대화상자를 연 뒤 시트가 닫혔거나 블록이 바뀌었으면 쓰지 않고 알린다.
    /// 되돌리기 한 번으로 돌아가고, 커서는 그대로 둔다.
    fn replace_extension_block(
        &mut self,
        edit: &EditBlock,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = &self.editor else {
            return;
        };
        if editor.state() != &edit.state {
            self.notify(t!("extensions.sheet_closed"), window, cx);
            return;
        }
        let current = edit.state.read(cx).value();
        if current.get(edit.range.clone()) != Some(edit.original.as_str()) {
            self.notify(t!("extensions.block_changed"), window, cx);
            return;
        }
        if text != edit.original {
            edit.state.update(cx, |state, cx| {
                state.replace_range_keeping_selections(edit.range.clone(), text, window, cx);
            });
            self.on_editor_changed(window, cx);
        }
    }

    /// 대화상자를 연 시트(`target`)가 아직 열려 있으면 커서 자리에 블록을 넣는다. 블록은 제 줄에 둔다.
    fn insert_extension(
        &mut self,
        target: &Entity<EditorState>,
        name: &str,
        args: &[(String, String)],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = &self.editor else {
            return;
        };
        if editor.state() != target {
            self.notify(t!("extensions.sheet_closed"), window, cx);
            return;
        }
        editor.state().update(cx, |state, cx| {
            let text = state.value();
            let at = state.selected_range().end.min(text.len());
            let block = extension::block_text(name, args);
            let mut inserted = String::new();
            if at > 0 && !text[..at].ends_with('\n') {
                inserted.push('\n');
            }
            inserted.push_str(&block);
            // 커서는 블록 다음 줄의 처음에 둔다. 커서가 블록 밖에 있어야 펜스가 접혀 그림이 보인다.
            let cursor = if text[at..].starts_with('\n') {
                at + inserted.len() + 1
            } else {
                inserted.push('\n');
                at + inserted.len()
            };
            state.set_selected_range(at..at, cx);
            state.replace(inserted, window, cx);
            state.set_selected_range(cursor..cursor, cx);
            state.focus(window, cx);
        });
        self.on_editor_changed(window, cx);
    }
}
