//! 책 스타일 화면(에디터 자리): 대상 목록 → 대상별 편집(+미리보기), 다른 프로젝트에서 가져오기.
//!
//! 값을 바꾸면 바로 프로젝트 `.hanji.toml`의 `[styles]`에 쓴다(ADR-0009). 에디터는 시트를 열 때
//! 스타일을 다시 읽는다.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::searchable_list::SearchableVec;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, FocusableExt as _, Icon, IndexPath, Sizable as _,
    StyledExt as _, WindowExt as _,
};
use gpui_kit::gpui::prelude::{FluentBuilder as _, InteractiveElement as _};
use gpui_kit::gpui::*;
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::HanjiApp;
use super::style::*;
use crate::icons;
use crate::library::{self, Align, Numbering, Placement, StyleTarget, VerticalAlign};
use crate::settings::{FontItem, Prefs, font_items};

/// 내용 최대 폭.
const CONTENT_MAX_WIDTH: f32 = 960.0;

/// 편집 폼 폭(나머지는 미리보기).
const FORM_WIDTH: f32 = 340.0;

/// 미리보기 최소 폭. 이보다 좁으면 폼 아래로 내린다.
const PREVIEW_MIN_WIDTH: f32 = 280.0;

/// 미리보기 기준 글자 크기(에디터 본문과 같다).
const PREVIEW_FONT_SIZE: f32 = 16.0;

/// 줄 간격 슬라이더 범위.
const LINE_HEIGHT_RANGE: (f32, f32) = (1.0, 2.5);

/// 별도 제목 페이지 미리보기 높이.
const PREVIEW_TITLE_PAGE_HEIGHT: f32 = 220.0;

/// 미리보기 본문(화면 글이라 UI 언어를 따른다).
fn sample_body() -> [Cow<'static, str>; 2] {
    [
        t!("book.style.sample.body_1"),
        t!("book.style.sample.body_2"),
    ]
}

/// 대상별 미리보기 제목. 접두사(`제{n}장 ` 등)는 스타일에서 붙인다.
fn sample_title(target: StyleTarget) -> Cow<'static, str> {
    match target {
        StyleTarget::Body => "".into(),
        StyleTarget::Part => t!("book.style.sample.part"),
        StyleTarget::Chapter => t!("book.style.sample.chapter"),
        StyleTarget::Section => t!("book.style.sample.section"),
    }
}

/// 글자 굵기 표시 이름.
fn weight_title(weight: u16) -> SharedString {
    match weight {
        400 => t!("book.style.weight.regular").into(),
        700 => t!("book.style.weight.bold").into(),
        _ => weight.to_string().into(),
    }
}

/// 숫자 칸 값 → 스타일 값. 비우면 기본값(`None`), 숫자가 아니면 바꾸지 않는다(`Err`).
fn parse_number(text: &str) -> Result<Option<f64>, ()> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    text.parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value >= 0.0)
        .map(Some)
        .ok_or(())
}

/// 숫자를 칸에 보일 문자열로(1.0 → "1", 1.25 → "1.25").
fn format_number(value: f64) -> String {
    let text = format!("{value:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// 숫자 칸이 고치는 값.
#[derive(Clone, Copy, PartialEq, Eq)]
enum NumberField {
    FontSize,
    FirstLineIndent,
    SpaceBefore,
    SpaceAfter,
}

impl NumberField {
    fn label(self, target: StyleTarget) -> Cow<'static, str> {
        match self {
            NumberField::FontSize => t!("book.style.font_size"),
            NumberField::FirstLineIndent => t!("book.style.first_line_indent"),
            NumberField::SpaceBefore => t!("book.style.space_before"),
            NumberField::SpaceAfter if target.is_title() => t!("book.style.space_after"),
            NumberField::SpaceAfter => t!("book.style.paragraph_spacing"),
        }
    }

    /// 대상의 편집 폼에 나오는 숫자 칸.
    fn for_target(target: StyleTarget) -> &'static [NumberField] {
        if target.is_title() {
            &[
                NumberField::FontSize,
                NumberField::SpaceBefore,
                NumberField::SpaceAfter,
            ]
        } else {
            &[NumberField::FirstLineIndent, NumberField::SpaceAfter]
        }
    }

    fn get(self, spec: &library::StyleSpec) -> Option<f64> {
        match self {
            NumberField::FontSize => spec.font_size,
            NumberField::FirstLineIndent => spec.first_line_indent,
            NumberField::SpaceBefore => spec.space_before,
            NumberField::SpaceAfter => spec.space_after,
        }
    }

    fn set(self, spec: &mut library::StyleSpec, value: Option<f64>) {
        match self {
            NumberField::FontSize => spec.font_size = value,
            NumberField::FirstLineIndent => spec.first_line_indent = value,
            NumberField::SpaceBefore => spec.space_before = value,
            NumberField::SpaceAfter => spec.space_after = value,
        }
    }

    fn resolved(self, style: &library::ResolvedStyle) -> f64 {
        match self {
            NumberField::FontSize => style.font_size,
            NumberField::FirstLineIndent => style.first_line_indent,
            NumberField::SpaceBefore => style.space_before,
            NumberField::SpaceAfter => style.space_after,
        }
    }
}

/// 대상 하나를 고치는 입력 상태. 대상을 열 때 만들고 목록으로 돌아가면 버린다.
struct TargetForm {
    target: StyleTarget,
    font: Entity<SelectState<SearchableVec<FontItem>>>,
    line_height: Entity<SliderState>,
    numbers: Vec<(NumberField, Entity<InputState>)>,
    /// 제목 접두사(제목 대상만). Enter로 줄을 바꿀 수 있다.
    prefix: Option<Entity<TextareaState>>,
    _subscriptions: Vec<Subscription>,
}

struct TransformForm {
    before: Entity<InputState>,
    after: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

/// 책 스타일 화면.
pub(super) struct BookStyleView {
    project: PathBuf,
    styles: library::Styles,
    /// 열어 둔 대상. 없으면 대상 목록.
    form: Option<TargetForm>,
    transform_form: Option<TransformForm>,
    /// 가져오기에서 고른 프로젝트.
    import_source: Option<PathBuf>,
    /// 가져올 프로젝트 목록을 라이브러리에서 읽는다.
    app: WeakEntity<HanjiApp>,
}

impl BookStyleView {
    fn new(app: WeakEntity<HanjiApp>, project: PathBuf) -> Self {
        Self {
            styles: library::read_styles(&project),
            project,
            form: None,
            transform_form: None,
            import_source: None,
            app,
        }
    }

    /// 이 화면이 보여 주는 프로젝트.
    pub(super) fn project(&self) -> &Path {
        &self.project
    }

    /// 스타일을 파일에 쓴다. 실패하면 알림을 띄운다.
    fn save(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(err) = library::write_styles(&self.project, &self.styles) {
            window.push_notification(
                Notification::info(t!("book.style.save_failed", err = err)),
                cx,
            );
        }
        cx.notify();
    }

    /// 대상 하나를 고치고 저장한다.
    fn update_spec(
        &mut self,
        target: StyleTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut library::StyleSpec),
    ) {
        change(self.styles.get_mut(target));
        self.save(window, cx);
    }

    fn open_transform(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let before = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("book.transform.before_placeholder"))
        });
        let after = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("book.transform.after_placeholder"))
        });
        let mut subscriptions = Vec::new();
        for input in [&before, &after] {
            subscriptions.push(cx.subscribe(input, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }));
        }
        self.transform_form = Some(TransformForm {
            before,
            after,
            _subscriptions: subscriptions,
        });
        cx.notify();
    }

    fn add_replacement(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = &self.transform_form else {
            return;
        };
        let before = form.before.read(cx).value().to_string();
        let after = form.after.read(cx).value().to_string();
        if before.is_empty() {
            return;
        }
        if self
            .styles
            .input_transform
            .replacements
            .iter()
            .any(|rule| rule.before == before)
        {
            window.push_notification(Notification::info(t!("book.transform.duplicate")), cx);
            return;
        }
        self.styles
            .input_transform
            .replacements
            .push(crate::text_transform::Replacement {
                before,
                after,
                enabled: true,
            });
        form.before
            .update(cx, |input, cx| input.set_value("", window, cx));
        form.after
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.save(window, cx);
    }

    fn render_transform(
        &self,
        form: &TransformForm,
        cx: &mut Context<Self>,
        pal: Palette,
    ) -> AnyElement {
        let options = &self.styles.input_transform;
        let controls = [
            (
                "transform-quotes",
                t!("book.transform.smart_quotes"),
                t!("book.transform.smart_quotes_description"),
                options.smart_quotes,
            ),
            (
                "transform-ellipsis",
                t!("book.transform.ellipsis"),
                "... → …".into(),
                options.ellipsis,
            ),
            (
                "transform-dash",
                t!("book.transform.dash"),
                "-- → —".into(),
                options.dash,
            ),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (id, label, description, checked))| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    Checkbox::new(id)
                        .label(SharedString::from(label))
                        .checked(checked)
                        .on_click(cx.listener(move |this, checked: &bool, window, cx| {
                            let options = &mut this.styles.input_transform;
                            match index {
                                0 => options.smart_quotes = *checked,
                                1 => options.ellipsis = *checked,
                                _ => options.dash = *checked,
                            }
                            this.save(window, cx);
                        })),
                )
                .child(div().text_xs().text_color(pal.muted).child(description))
        });
        let rules = options
            .replacements
            .iter()
            .enumerate()
            .map(|(index, rule)| {
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        Checkbox::new(SharedString::from(format!("replacement-enabled-{index}")))
                            .label(SharedString::from(t!("book.transform.enabled")))
                            .checked(rule.enabled)
                            .on_click(cx.listener(move |this, checked: &bool, window, cx| {
                                if let Some(rule) =
                                    this.styles.input_transform.replacements.get_mut(index)
                                {
                                    rule.enabled = *checked;
                                    this.save(window, cx);
                                }
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .whitespace_normal()
                            .child(rule.before.clone()),
                    )
                    .child(div().text_color(pal.muted).child("→"))
                    .child(
                        div()
                            .flex_1()
                            .whitespace_normal()
                            .child(if rule.after.is_empty() {
                                t!("book.transform.deletes").into_owned()
                            } else {
                                rule.after.clone()
                            }),
                    )
                    .child(
                        Button::new(SharedString::from(format!("replacement-remove-{index}")))
                            .ghost()
                            .small()
                            .label(t!("common.delete"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if index < this.styles.input_transform.replacements.len() {
                                    this.styles.input_transform.replacements.remove(index);
                                    this.save(window, cx);
                                }
                            })),
                    )
            });
        let can_add = !form.before.read(cx).value().is_empty();
        let back = Button::new("transform-back")
            .ghost()
            .small()
            .icon(Icon::new(icons::leave()))
            .label(t!("book.style.back_to_list"))
            .on_click(cx.listener(|this, _, _, cx| {
                this.transform_form = None;
                cx.notify();
            }));
        div()
            .flex()
            .flex_col()
            .gap_6()
            .child(Self::header(
                pal,
                t!("book.transform.title"),
                t!("book.transform.description"),
                Some(back.into_any_element()),
            ))
            .child(
                div()
                    .p_5()
                    .rounded_xl()
                    .border_1()
                    .border_color(pal.panel_border)
                    .bg(pal.panel)
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(
                        div()
                            .font_semibold()
                            .child(t!("book.transform.smart_punctuation")),
                    )
                    .children(controls),
            )
            .child(
                div()
                    .p_5()
                    .rounded_xl()
                    .border_1()
                    .border_color(pal.panel_border)
                    .bg(pal.panel)
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(div().font_semibold().child(t!("book.transform.custom")))
                    .child(
                        div()
                            .text_xs()
                            .text_color(pal.muted)
                            .child(t!("book.transform.custom_description")),
                    )
                    .when(options.replacements.is_empty(), |this| {
                        this.child(
                            div()
                                .text_sm()
                                .text_color(pal.muted)
                                .child(t!("book.transform.no_rules")),
                        )
                    })
                    .children(rules)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(Input::new(&form.before).focus_ring(false)),
                            )
                            .child(div().text_color(pal.muted).child("→"))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(Input::new(&form.after).focus_ring(false)),
                            ),
                    )
                    .child(
                        Button::new("replacement-add")
                            .label(t!("book.transform.add_rule"))
                            .disabled(!can_add)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.add_replacement(window, cx)),
                            ),
                    ),
            )
            .child(
                div()
                    .p_5()
                    .rounded_xl()
                    .border_1()
                    .border_color(pal.panel_border)
                    .bg(pal.panel)
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        Checkbox::new("transform-on-paste")
                            .label(SharedString::from(t!("book.transform.on_paste")))
                            .checked(options.apply_on_paste)
                            .on_click(cx.listener(|this, checked: &bool, window, cx| {
                                this.styles.input_transform.apply_on_paste = *checked;
                                this.save(window, cx);
                            })),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(pal.muted)
                            .child(t!("book.transform.on_paste_description")),
                    ),
            )
            .into_any_element()
    }

    /// 대상 편집을 연다(입력 상태를 지금 값으로 만든다).
    fn open_target(&mut self, target: StyleTarget, window: &mut Window, cx: &mut Context<Self>) {
        let spec = self.styles.get(target).clone();
        let resolved = spec.resolve(target);
        let mut subscriptions = Vec::new();

        // 제목은 서체를 정하지 않으면 본문 서체를 따른다.
        let items = if target.is_title() {
            font_items(
                t!("book.style.font.inherit"),
                self.effective_font(StyleTarget::Body, cx),
                cx,
            )
        } else {
            font_items(
                t!("book.style.font.default"),
                Prefs::editor_font_family(cx),
                cx,
            )
        };
        let selected = items
            .iter()
            .position(|item| item.value.as_deref() == spec.font.as_deref())
            .unwrap_or(0);
        let font = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(items),
                Some(IndexPath::new(selected)),
                window,
                cx,
            )
            .searchable(true)
        });
        subscriptions.push(cx.subscribe_in(
            &font,
            window,
            move |this, _, event: &SelectEvent<SearchableVec<FontItem>>, window, cx| {
                let SelectEvent::Confirm(Some(font)) = event else {
                    return;
                };
                let font = font.as_ref().map(|font| font.to_string());
                this.update_spec(target, window, cx, |spec| spec.font = font);
            },
        ));

        let (min, max) = LINE_HEIGHT_RANGE;
        let line_height = cx.new(|_| {
            SliderState::new()
                .min(min)
                .max(max)
                .step(0.05)
                .default_value(resolved.line_height as f32)
        });
        // 끄는 동안에는 미리보기만, 놓을 때 저장한다.
        subscriptions.push(cx.subscribe_in(
            &line_height,
            window,
            move |this, _, event: &SliderEvent, window, cx| match event {
                SliderEvent::Change(value) => {
                    this.styles.get_mut(target).line_height = Some(f64::from(value.end()));
                    cx.notify();
                }
                SliderEvent::Release(value) => {
                    let value = f64::from(value.end());
                    this.update_spec(target, window, cx, |spec| spec.line_height = Some(value));
                }
            },
        ));

        let numbers = NumberField::for_target(target)
            .iter()
            .map(|&field| {
                let input = cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(format_number(field.resolved(&target.defaults())))
                        .default_value(field.get(&spec).map(format_number).unwrap_or_default())
                });
                subscriptions.push(cx.subscribe_in(
                    &input,
                    window,
                    move |this, input, event: &InputEvent, window, cx| {
                        if !matches!(event, InputEvent::Change) {
                            return;
                        }
                        if let Ok(value) = parse_number(&input.read(cx).value()) {
                            this.update_spec(target, window, cx, |spec| field.set(spec, value));
                        }
                    },
                ));
                (field, input)
            })
            .collect();

        let prefix = target.is_title().then(|| {
            let input = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder(t!("book.style.prefix_placeholder"))
                    .auto_grow(1, 4)
                    .default_value(spec.prefix.clone().unwrap_or_default())
            });
            subscriptions.push(cx.subscribe_in(
                &input,
                window,
                move |this, input, event: &InputEvent, window, cx| {
                    if !matches!(event, InputEvent::Change) {
                        return;
                    }
                    let value = input.read(cx).value().to_string();
                    let value = (!value.is_empty()).then_some(value);
                    this.update_spec(target, window, cx, |spec| spec.prefix = value);
                },
            ));
            input
        });

        self.form = Some(TargetForm {
            target,
            font,
            line_height,
            numbers,
            prefix,
            _subscriptions: subscriptions,
        });
        cx.notify();
    }

    /// 대상이 실제로 쓰는 서체: 지정 서체 → (제목이면) 본문 서체 → 설정의 에디터 서체.
    fn effective_font(&self, target: StyleTarget, cx: &App) -> SharedString {
        self.styles
            .resolve(target)
            .font
            .map_or_else(|| Prefs::editor_font_family(cx), SharedString::from)
    }

    /// 서체 포함을 바꾼다. 서체를 정하지 않은 채 켜면 지금 쓰는 서체를 적어 넣는다.
    /// 설정의 에디터 서체는 컴퓨터마다 달라서, 그대로 두면 내보내는 곳마다 결과가 달라진다.
    fn set_embed(
        &mut self,
        target: StyleTarget,
        embed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pin = (embed && self.styles.resolve(target).font.is_none())
            .then(|| self.effective_font(target, cx).to_string());
        let pinned = pin.is_some();
        self.update_spec(target, window, cx, |spec| {
            spec.embed_font = Some(embed);
            if let Some(font) = pin {
                spec.font = Some(font);
            }
        });
        if pinned {
            // 서체 칸에 적어 넣은 서체를 보인다.
            self.open_target(target, window, cx);
        }
    }

    /// 대상을 기본값으로 되돌리고 입력 칸도 다시 만든다.
    fn reset_target(&mut self, target: StyleTarget, window: &mut Window, cx: &mut Context<Self>) {
        self.update_spec(target, window, cx, |spec| *spec = Default::default());
        self.open_target(target, window, cx);
    }

    /// 고른 프로젝트의 스타일을 그대로 가져온다(이 프로젝트의 스타일은 덮어쓴다).
    fn import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(source) = self.import_source.clone() else {
            return;
        };
        match library::import_styles(&source, &self.project) {
            Ok(styles) => {
                self.styles = styles;
                self.import_source = None;
                window.push_notification(Notification::info(t!("book.style.imported")), cx);
            }
            Err(err) => window.push_notification(
                Notification::info(t!("book.style.import_failed", err = err)),
                cx,
            ),
        }
        cx.notify();
    }

    /// 가져올 수 있는 프로젝트(이 프로젝트 말고): (경로, 이름).
    fn other_projects(&self, cx: &App) -> Vec<(PathBuf, String)> {
        let Some(app) = self.app.upgrade() else {
            return Vec::new();
        };
        let Some(lib) = &app.read(cx).library else {
            return Vec::new();
        };
        lib.section(library::SectionKind::Projects)
            .into_iter()
            .flat_map(|section| &section.entries)
            .filter_map(|entry| match entry {
                library::Entry::Group(group) if group.path != self.project => {
                    Some((group.path.clone(), group.name.clone()))
                }
                _ => None,
            })
            .collect()
    }

    /// 머리글: 작은 제목, 큰 제목, 설명, (오른쪽) 추가 버튼.
    fn header(
        pal: Palette,
        title: impl Into<SharedString>,
        description: impl Into<SharedString>,
        trailing: Option<AnyElement>,
    ) -> impl IntoElement {
        div()
            .flex()
            .items_end()
            .justify_between()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(pal.muted)
                            .child(t!("book.style.title")),
                    )
                    .child(div().text_3xl().font_semibold().child(title.into()))
                    .child(
                        div()
                            .text_sm()
                            .text_color(pal.muted)
                            .child(description.into()),
                    ),
            )
            .children(trailing)
    }

    /// 대상 목록 + 가져오기.
    fn render_list(&self, cx: &mut Context<Self>, pal: Palette) -> AnyElement {
        let cards = StyleTarget::ALL.into_iter().map(|target| {
            div()
                .id(target.key())
                .px_5()
                .py_4()
                .flex()
                .items_center()
                .gap_4()
                .rounded_xl()
                .border_1()
                .border_color(pal.panel_border)
                .bg(pal.panel)
                .cursor_pointer()
                .hover(move |style| style.bg(pal.hover))
                .on_click(
                    cx.listener(move |this, _, window, cx| this.open_target(target, window, cx)),
                )
                .child(
                    div()
                        .size(px(36.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_lg()
                        .bg(pal.background)
                        .child(Icon::new(icons::paragraph()).text_color(pal.foreground)),
                )
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .child(div().font_semibold().child(target.title()))
                        .child(div().text_xs().text_color(pal.muted).child(target.key())),
                )
                .child(Icon::new(icons::enter()).text_color(pal.muted))
        });

        let view = cx.entity().downgrade();
        let projects = self.other_projects(cx);
        let source_name = self.import_source.as_ref().and_then(|source| {
            projects
                .iter()
                .find(|(path, _)| path == source)
                .map(|(_, name)| name.clone())
        });
        let picker = Button::new("style-import-source")
            .label(source_name.unwrap_or_else(|| t!("book.style.import_pick").into()))
            .dropdown_caret(true)
            .disabled(projects.is_empty())
            .w_full()
            .dropdown_menu(move |menu, _, _| {
                projects
                    .iter()
                    .fold(menu.min_w(px(240.0)), |menu, (path, name)| {
                        let (view, path) = (view.clone(), path.clone());
                        menu.item(PopupMenuItem::new(name.clone()).on_click(move |_, _, cx| {
                            view.update(cx, |view, cx| {
                                view.import_source = Some(path.clone());
                                cx.notify();
                            })
                            .ok();
                        }))
                    })
            });
        let import = div()
            .pt_6()
            .border_t_1()
            .border_color(pal.panel_border)
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .text_color(pal.muted)
                    .child(t!("book.style.import_description")),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().min_w_0().child(picker))
                    .child(
                        Button::new("style-import")
                            .label(t!("book.style.apply"))
                            .disabled(self.import_source.is_none())
                            .on_click(cx.listener(|this, _, window, cx| this.import(window, cx))),
                    ),
            );

        div()
            .flex()
            .flex_col()
            .gap_6()
            .child(Self::header(
                pal,
                t!("book.style.title"),
                t!("book.style.list_description"),
                None,
            ))
            .child(div().flex().flex_col().gap_3().children(cards))
            .child(
                div()
                    .id("style-input-transform")
                    .px_5()
                    .py_4()
                    .flex()
                    .items_center()
                    .gap_4()
                    .rounded_xl()
                    .border_1()
                    .border_color(pal.panel_border)
                    .bg(pal.panel)
                    .cursor_pointer()
                    .hover(move |style| style.bg(pal.hover))
                    .on_click(cx.listener(|this, _, window, cx| this.open_transform(window, cx)))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().font_semibold().child(t!("book.transform.title")))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(pal.muted)
                                    .child(t!("book.transform.summary")),
                            ),
                    )
                    .child(Icon::new(icons::enter()).text_color(pal.muted)),
            )
            .child(import)
            .into_any_element()
    }

    /// 대상 편집: 왼쪽 폼, 오른쪽 미리보기, 아래 기본값으로 되돌리기.
    fn render_form(&self, form: &TargetForm, cx: &mut Context<Self>, pal: Palette) -> AnyElement {
        let target = form.target;
        let spec = self.styles.get(target);
        let resolved = self.styles.resolve(target);
        let view = cx.entity().downgrade();

        let field = |label: SharedString, control: AnyElement| {
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(div().text_sm().text_color(pal.muted).child(label))
                .child(control)
        };

        let embed = choice(
            "style-embed-font",
            &view,
            resolved.embed_font,
            [
                (false, t!("book.style.embed.system").into()),
                (true, t!("book.style.embed.file").into()),
            ],
            220.0,
            move |view, embed, window, cx| view.set_embed(target, embed, window, cx),
        );
        let align = choice(
            "style-align",
            &view,
            resolved.align,
            Align::ALL.map(|align| (align, align.title().into())),
            160.0,
            move |view, align, window, cx| {
                view.update_spec(target, window, cx, |spec| spec.align = Some(align))
            },
        );
        let number = |wanted: NumberField| {
            form.numbers
                .iter()
                .find(|(number, _)| *number == wanted)
                .map(|(number, input)| {
                    field(
                        number.label(target).into(),
                        Input::new(input).focus_ring(false).into_any_element(),
                    )
                })
        };
        let font = field(
            t!("book.style.font").into(),
            Select::new(&form.font)
                .focus_ring(false)
                .menu_max_h(px(320.0))
                .search_placeholder(t!("settings.font.search"))
                .into_any_element(),
        );
        let embed = field(t!("book.style.embed").into(), embed.into_any_element());
        let line_height = field(
            t!(
                "book.style.line_height",
                value = format_number(resolved.line_height)
            )
            .into(),
            Slider::new(&form.line_height).into_any_element(),
        );

        let fields = div()
            .w(px(FORM_WIDTH))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_5();
        let fields = if target.is_title() {
            // 제목: 배치 → 접두사 → 글자 → 배치에 따른 위치·여백(epubly 스타일 화면과 같은 순서).
            let inline = resolved.placement == Placement::Inline;
            let page = resolved.placement == Placement::Page;
            let placement = choice(
                "style-placement",
                &view,
                resolved.placement,
                Placement::ALL.map(|placement| (placement, placement.title().into())),
                200.0,
                move |view, placement, window, cx| {
                    view.update_spec(target, window, cx, |spec| spec.placement = Some(placement))
                },
            );
            let page_break = choice(
                "style-page-break",
                &view,
                resolved.page_break,
                [
                    (true, t!("book.style.page_break.always").into()),
                    (false, t!("book.style.page_break.continue").into()),
                ],
                200.0,
                move |view, page_break, window, cx| {
                    view.update_spec(target, window, cx, |spec| {
                        spec.page_break = Some(page_break)
                    })
                },
            );
            let weight = choice(
                "style-font-weight",
                &view,
                resolved.font_weight,
                library::FONT_WEIGHTS.map(|weight| (weight, weight_title(weight))),
                160.0,
                move |view, weight, window, cx| {
                    view.update_spec(target, window, cx, |spec| spec.font_weight = Some(weight))
                },
            );
            let vertical = choice(
                "style-vertical-align",
                &view,
                resolved.vertical_align,
                VerticalAlign::ALL.map(|align| (align, align.title().into())),
                160.0,
                move |view, align, window, cx| {
                    view.update_spec(target, window, cx, |spec| spec.vertical_align = Some(align))
                },
            );
            let prefix = form.prefix.as_ref().map(|input| {
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(pal.muted)
                            .child(t!("book.style.prefix")),
                    )
                    .child(Textarea::new(input))
                    .child(div().text_xs().text_color(pal.muted).child(t!(
                        "book.style.prefix_hint",
                        number = library::PREFIX_NUMBER
                    )))
            });
            let numbering = choice(
                "style-numbering",
                &view,
                resolved.numbering,
                Numbering::ALL.map(|numbering| (numbering, numbering.title().into())),
                160.0,
                move |view, numbering, window, cx| {
                    view.update_spec(target, window, cx, |spec| spec.numbering = Some(numbering))
                },
            );
            // 부에는 상위가 없으니 장·절만 고른다.
            let restart = (target != StyleTarget::Part).then(|| {
                choice(
                    "style-restart-on-parent",
                    &view,
                    resolved.restart_on_parent,
                    [
                        (false, t!("book.style.restart.continue").into()),
                        (true, t!("book.style.restart.from_one").into()),
                    ],
                    160.0,
                    move |view, restart, window, cx| {
                        view.update_spec(target, window, cx, |spec| {
                            spec.restart_on_parent = Some(restart)
                        })
                    },
                )
            });
            let in_toc = Checkbox::new("style-in-toc")
                .label(SharedString::from(t!("book.style.in_toc")))
                .checked(resolved.in_toc)
                .on_click(cx.listener(move |this, checked: &bool, window, cx| {
                    let in_toc = *checked;
                    this.update_spec(target, window, cx, |spec| spec.in_toc = Some(in_toc));
                }));
            fields
                .child(field(
                    t!("book.style.placement").into(),
                    placement.into_any_element(),
                ))
                .when(!page, |this| {
                    this.child(field(
                        t!("book.style.page_break").into(),
                        page_break.into_any_element(),
                    ))
                })
                .child(in_toc)
                // 목차에 넣지 않아도 번호·접두사는 넣을 수 있다.
                .child(field(
                    t!("book.style.numbering").into(),
                    numbering.into_any_element(),
                ))
                .children(restart.map(|restart| {
                    field(t!("book.style.restart").into(), restart.into_any_element())
                }))
                .children(prefix)
                .child(font)
                .child(embed)
                .children(number(NumberField::FontSize))
                .child(field(
                    t!("book.style.weight").into(),
                    weight.into_any_element(),
                ))
                .child(line_height)
                .child(field(
                    t!("book.style.align").into(),
                    align.into_any_element(),
                ))
                .when(page, |this| {
                    this.child(field(
                        t!("book.style.vertical_align").into(),
                        vertical.into_any_element(),
                    ))
                })
                .when(inline && !resolved.page_break, |this| {
                    this.children(number(NumberField::SpaceBefore))
                })
                .when(inline, |this| {
                    this.children(number(NumberField::SpaceAfter))
                })
        } else {
            fields
                .child(
                    Checkbox::new("style-preserve-line-breaks")
                        .label(SharedString::from(t!("book.style.preserve_line_breaks")))
                        .checked(resolved.preserve_line_breaks)
                        .on_click(cx.listener(|this, checked: &bool, window, cx| {
                            this.update_spec(StyleTarget::Body, window, cx, |spec| {
                                spec.preserve_line_breaks = Some(*checked);
                            });
                        })),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(pal.muted)
                        .child(t!("book.style.preserve_line_breaks_description")),
                )
                .child(font)
                .child(embed)
                .child(line_height)
                .child(field(
                    t!("book.style.paragraph_align").into(),
                    align.into_any_element(),
                ))
                .children(number(NumberField::FirstLineIndent))
                .children(number(NumberField::SpaceAfter))
        };

        let preview = div()
            .flex_1()
            .min_w(px(PREVIEW_MIN_WIDTH))
            .overflow_hidden()
            .p_6()
            .rounded_xl()
            .border_1()
            .border_color(pal.panel_border)
            .bg(pal.panel)
            .flex()
            .flex_col()
            .child(
                div()
                    .pb_4()
                    .text_xs()
                    .text_color(pal.muted)
                    .child(t!("book.preview.title")),
            )
            .child(self.render_preview(target, cx))
            .when(!resolved.embed_font, |this| {
                this.child(
                    div()
                        .pt_4()
                        .text_xs()
                        .text_color(pal.muted)
                        .child(t!("book.style.embed_hint")),
                )
            });

        let back = Button::new("style-back")
            .ghost()
            .small()
            .icon(Icon::new(icons::leave()))
            .label(t!("book.style.back_to_list"))
            .on_click(cx.listener(|this, _, _, cx| {
                this.form = None;
                cx.notify();
            }));
        let description = t!("book.style.edit_description");

        div()
            .flex()
            .flex_col()
            .gap_6()
            .child(Self::header(
                pal,
                target.title(),
                description,
                Some(back.into_any_element()),
            ))
            .child(
                // 좁으면 미리보기를 폼 아래로 내린다.
                div()
                    .flex()
                    .flex_wrap()
                    .items_start()
                    .gap_6()
                    .child(fields)
                    .child(preview),
            )
            .child(
                div()
                    .pt_4()
                    .border_t_1()
                    .border_color(pal.panel_border)
                    .flex()
                    .justify_end()
                    .child(
                        Button::new("style-reset")
                            .ghost()
                            .label(t!("book.style.reset"))
                            .disabled(spec.is_empty())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.reset_target(target, window, cx)
                            })),
                    ),
            )
            .into_any_element()
    }

    /// 미리보기: 내보낸 책에서 보일 모습. 제목 대상이면 배치에 따라 제목·쪽 나눔·본문을 그린다.
    ///
    /// 서체를 넣지 않으면 리더 기본 서체로 보이므로 시스템 서체로 그린다.
    /// 화면 텍스트는 양쪽 정렬이 없어 왼쪽 정렬로, 첫 줄 들여쓰기는 앞 공백으로 흉내 낸다.
    fn render_preview(&self, target: StyleTarget, cx: &App) -> impl IntoElement {
        let pal = Palette::from_theme(cx.theme());
        let reader_family = cx.theme().font_family.clone();
        let paragraph = |style: &library::ResolvedStyle, family: SharedString, text: String| {
            let size = PREVIEW_FONT_SIZE * style.font_size as f32;
            let family = if style.embed_font {
                family
            } else {
                reader_family.clone()
            };
            let first_indent = indent(style.first_line_indent);
            div()
                .w_full()
                .whitespace_normal()
                .font_family(family)
                .text_size(px(size))
                .font_weight(FontWeight(f32::from(style.font_weight)))
                .line_height(relative(style.line_height as f32))
                .pt(px(PREVIEW_FONT_SIZE * style.space_before as f32))
                .pb(px(PREVIEW_FONT_SIZE * style.space_after as f32))
                .map(|this| match style.align {
                    Align::Center => this.text_center(),
                    Align::Right => this.text_right(),
                    Align::Left | Align::Justify => this.text_left(),
                })
                // 접두사에 넣은 줄바꿈(Enter)을 줄마다 따로 그린다.
                .children(text.split('\n').enumerate().map(move |(ix, line)| {
                    let lead = if ix == 0 {
                        first_indent.clone()
                    } else {
                        String::new()
                    };
                    div()
                        .min_h(px(size * style.line_height as f32))
                        .child(format!("{lead}{line}"))
                }))
        };
        let body = self.styles.resolve(StyleTarget::Body);
        let body_family = self.effective_font(StyleTarget::Body, cx);
        let body_text = |text: &str| {
            let text = if body.preserve_line_breaks {
                text.to_string()
            } else {
                text.replace('\n', " ")
            };
            paragraph(&body, body_family.clone(), text)
        };
        let page_break = || {
            div()
                .my_5()
                .flex()
                .items_center()
                .gap_2()
                .text_xs()
                .text_color(pal.muted)
                .child(div().h(px(1.0)).flex_1().bg(pal.panel_border))
                .child(t!("book.style.page_break_marker"))
                .child(div().h(px(1.0)).flex_1().bg(pal.panel_border))
        };

        let mut blocks: Vec<AnyElement> = Vec::new();
        let title = target
            .is_title()
            .then(|| self.styles.resolve(target))
            .filter(|title| title.placement != Placement::Hidden);
        let Some(title) = title else {
            for (index, text) in sample_body().into_iter().enumerate() {
                if index > 0 && body.preserve_line_breaks {
                    blocks.push(
                        div()
                            .h(px(PREVIEW_FONT_SIZE
                                * body.font_size as f32
                                * body.line_height as f32))
                            .into_any_element(),
                    );
                }
                blocks.push(body_text(&text).into_any_element());
            }
            return div().flex().flex_col().children(blocks);
        };
        let heading = paragraph(
            &title,
            self.effective_font(target, cx),
            title.title_text(1, &sample_title(target)),
        );
        let [sample_1, sample_2] = sample_body();
        match title.placement {
            Placement::Page => {
                // 쪽 위·아래 여백 없이 세로 위치만 따른다.
                let heading = heading.pt_0().pb_0();
                blocks.push(
                    div()
                        .h(px(PREVIEW_TITLE_PAGE_HEIGHT))
                        .flex()
                        .flex_col()
                        .map(|this| match title.vertical_align {
                            VerticalAlign::Top => this.justify_start(),
                            VerticalAlign::Center => this.justify_center(),
                            VerticalAlign::Bottom => this.justify_end(),
                        })
                        .child(heading)
                        .into_any_element(),
                );
                blocks.push(page_break().into_any_element());
                blocks.push(body_text(&sample_1).into_any_element());
            }
            _ if title.page_break => {
                // 새 페이지 첫머리라 위 여백은 없다.
                blocks.push(page_break().into_any_element());
                blocks.push(heading.pt_0().into_any_element());
                blocks.push(body_text(&sample_1).into_any_element());
            }
            _ => {
                blocks.push(body_text(&sample_2).into_any_element());
                blocks.push(heading.into_any_element());
                blocks.push(body_text(&sample_1).into_any_element());
            }
        }
        div().flex().flex_col().children(blocks)
    }
}

/// 값 하나를 고르는 드롭다운 버튼. 고르면 `pick`을 부른다.
fn choice<T: Copy + PartialEq + 'static>(
    id: &'static str,
    view: &WeakEntity<BookStyleView>,
    current: T,
    options: impl IntoIterator<Item = (T, SharedString)>,
    menu_width: f32,
    pick: impl Fn(&mut BookStyleView, T, &mut Window, &mut Context<BookStyleView>) + 'static,
) -> impl IntoElement {
    let options: Vec<_> = options.into_iter().collect();
    let label = options
        .iter()
        .find(|(value, _)| *value == current)
        .map(|(_, title)| title.clone())
        .unwrap_or_default();
    let (view, pick) = (view.clone(), Rc::new(pick));
    Button::new(id)
        .label(label)
        .dropdown_caret(true)
        .w_full()
        .dropdown_menu(move |menu, _, _| {
            options
                .iter()
                .fold(menu.min_w(px(menu_width)), |menu, (value, title)| {
                    let (view, pick, value) = (view.clone(), pick.clone(), *value);
                    menu.item(
                        PopupMenuItem::new(title.clone())
                            .checked(value == current)
                            .on_click(move |_, window, cx| {
                                view.update(cx, |view, cx| pick(view, value, window, cx))
                                    .ok();
                            }),
                    )
                })
        })
}

/// 첫 줄 들여쓰기(em)를 공백 문자로: 전각 공백(1em) + 반각 공백(0.5em).
fn indent(em: f64) -> String {
    let halves = (em * 2.0).round().max(0.0) as usize;
    let mut text = "\u{3000}".repeat(halves / 2);
    if halves % 2 == 1 {
        text.push('\u{2002}');
    }
    text
}

impl Render for BookStyleView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = Palette::from_theme(cx.theme());
        let content = match (&self.transform_form, &self.form) {
            (Some(form), _) => self.render_transform(form, cx, pal),
            (_, Some(form)) => self.render_form(form, cx, pal),
            _ => self.render_list(cx, pal),
        };
        div()
            .flex_1()
            .min_h_0()
            .w_full()
            .px_10()
            .py_8()
            .child(div().max_w(px(CONTENT_MAX_WIDTH)).mx_auto().child(content))
            .overflow_y_scrollbar()
    }
}

impl HanjiApp {
    /// 에디터 자리에 프로젝트의 책 스타일 화면을 연다. 열린 시트는 저장하고 닫는다.
    pub(super) fn open_book_style(
        &mut self,
        dir: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_sheet(None, false, window, cx);
        if self.selected_sheet.is_some() {
            // 시트를 저장하지 못해 그대로 열려 있다.
            return;
        }
        let (app, project) = (cx.entity().downgrade(), dir.to_path_buf());
        self.book_style = Some(cx.new(|_| BookStyleView::new(app, project)));
        cx.notify();
    }
}
