//! 오른쪽 어시스턴트(LLM) 패널. 에디터 툴바 오른쪽 끝 버튼으로 열고 닫는다.
//!
//! 새 대화는 설정의 제공자(Bedrock·Token Plan)로 실제 모델과 주고받고, 답을 받는 대로 조금씩 그린다
//! ([`crate::agent`]). 대화 상태와 프롬프트는 에이전트가 맡고, 여기서는 그리기만 한다. 답이 끝날 때마다 대화를
//! 라이브러리의 대화 기록(redb, ADR-0031)에 쓰고, 기록 화면에서 지난 대화를 다시 열어 이어 간다.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Datelike as _, Local, NaiveDate, TimeZone as _};
use futures::StreamExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::searchable_list::{SearchableListItem, SearchableVec};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::*;
use gpui_kit::gpui::prelude::FluentBuilder;
use gpui_kit::gpui::*;

use super::HanjiApp;
use super::style::*;
use crate::{agent, icons};

/// 새 대화(실제 모델과 주고받는 대화)의 상태.
pub(super) struct LiveChat {
    /// 입력창. Enter로 보내고 Shift+Enter로 줄을 바꾼다.
    input: Entity<TextareaState>,
    conversation: agent::Conversation,
    /// 기록에 쓴 대화의 요약과 쓴 말 수. 아직 쓰지 않은 새 대화면 `None`.
    saved: Option<Saved>,
    /// 기록 화면에 보일 대화 요약(최근 것부터). 기록을 열 때 읽는다.
    summaries: Vec<agent::Summary>,
    /// 마지막으로 시작한 기록 쓰기. 앞의 쓰기가 끝난 뒤에 쓴다(늦게 끝난 옛 쓰기가 새 말을 지우지 않게).
    save_task: Option<Task<()>>,
    /// 답을 받는 태스크. 버리면 받기를 멈춘다.
    reply: Option<Task<()>>,
    /// 답이 자랄 때 맨 아래로 따라가는 스크롤.
    scroll: ScrollHandle,
    /// 승인 카드마다 diff 칸의 스크롤. 넘칠 때만 휠을 가져가 대화가 같이 움직이지 않게 한다.
    change_scrolls: RefCell<HashMap<u64, ScrollHandle>>,
    /// 입력창 아래의 모델 고르기. 리전에서 쓸 수 있는 모델만 담는다.
    model_picker: Entity<SelectState<SearchableVec<ModelItem>>>,
    models: ModelLookup,
    /// 모델 목록이 어느 설정([`agent::Backend::models_key`]) 것인지. 설정이 바뀌면 다시 확인한다.
    models_for: Option<String>,
    /// 모델 목록을 확인하는 태스크. 새로 시작하면 이전 것은 버린다.
    model_lookup: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

/// 리전에서 쓸 수 있는 모델 확인 상태.
enum ModelLookup {
    /// AI를 설정하지 않았다.
    Idle,
    Loading,
    /// 쓸 수 있는 모델과 부를 ID(Bedrock이면 추론 프로필).
    Ready(Vec<(agent::ModelChoice, String)>),
    Failed(SharedString),
}

/// 모델 고르기의 선택지 하나. 값은 부를 ID다. OCR 텍스트 삽입의 모델 고르기도 쓴다.
#[derive(Clone)]
pub(super) struct ModelItem {
    pub(super) title: SharedString,
    pub(super) id: SharedString,
}

impl SearchableListItem for ModelItem {
    type Value = SharedString;

    fn title(&self) -> SharedString {
        self.title.clone()
    }

    fn render(&self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div().child(self.title.clone())
    }

    fn value(&self) -> &SharedString {
        &self.id
    }
}

impl LiveChat {
    pub(super) fn new(window: &mut Window, cx: &mut Context<HanjiApp>) -> Self {
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 6)
                .submit_on_enter(true)
                .placeholder(t!("assistant.input.placeholder"))
        });
        let subscription =
            cx.subscribe_in(
                &input,
                window,
                |this, input, event, window, cx| match event {
                    InputEvent::PressEnter { shift: false, .. } => {
                        let text = input.read(cx).value().to_string();
                        this.send_assistant_message(text, window, cx);
                    }
                    // 보내기 버튼을 켜고 끄려고 다시 그린다.
                    InputEvent::Change => cx.notify(),
                    _ => {}
                },
            );
        let model_picker = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(Vec::<ModelItem>::new()),
                None,
                window,
                cx,
            )
        });
        let model_subscription = cx.subscribe(
            &model_picker,
            |_, _, event: &SelectEvent<SearchableVec<ModelItem>>, cx| {
                let SelectEvent::Confirm(value) = event;
                if let Some(id) = value {
                    crate::settings::Prefs::set_ai_model(Some(id.clone()), cx);
                }
            },
        );
        Self {
            input,
            conversation: agent::Conversation::default(),
            saved: None,
            summaries: Vec::new(),
            save_task: None,
            reply: None,
            scroll: ScrollHandle::new(),
            change_scrolls: RefCell::default(),
            model_picker,
            models: ModelLookup::Idle,
            models_for: None,
            model_lookup: None,
            _subscriptions: vec![subscription, model_subscription],
        }
    }
}

/// 기록에 쓴 대화.
struct Saved {
    summary: agent::Summary,
    /// 기록에 쓴 말 수. 다음에는 그 뒤의 말만 쓴다.
    persisted: usize,
}

/// 대화 기록에서 묶는 기간.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Period {
    Today,
    Yesterday,
    ThisWeek,
    Earlier,
}

impl Period {
    /// `at`이 `today`(지역 날짜)에서 며칠 전인지로 묶는다.
    fn of(at: DateTime<Local>, today: NaiveDate) -> Self {
        match (today - at.date_naive()).num_days() {
            ..=0 => Period::Today,
            1 => Period::Yesterday,
            2..=6 => Period::ThisWeek,
            _ => Period::Earlier,
        }
    }

    fn label(self) -> std::borrow::Cow<'static, str> {
        match self {
            Period::Today => t!("assistant.history.today"),
            Period::Yesterday => t!("assistant.history.yesterday"),
            Period::ThisWeek => t!("assistant.history.last_7_days"),
            Period::Earlier => t!("assistant.history.earlier"),
        }
    }
}

/// 유닉스 밀리초를 지역 시각으로.
fn local_time(millis: u64) -> Option<DateTime<Local>> {
    Local
        .timestamp_millis_opt(i64::try_from(millis).ok()?)
        .single()
}

/// 기록 줄 오른쪽에 적을 때. 오늘이면 시각, 올해면 월·일, 아니면 연·월·일.
fn time_label(at: DateTime<Local>, today: NaiveDate) -> String {
    if at.date_naive() == today {
        at.format("%H:%M").to_string()
    } else if at.year() == today.year() {
        t!("assistant.history.date", month = at.month(), day = at.day()).into()
    } else {
        t!(
            "assistant.history.date_year",
            year = at.year(),
            month = at.month(),
            day = at.day()
        )
        .into()
    }
}

/// 빈 대화에서 보여 줄 질문 예시(번역 키). 누르면 그 언어의 글을 보낸다.
const SUGGESTIONS: &[&str] = &[
    "assistant.suggestion.summarize",
    "assistant.suggestion.fix_awkward",
    "assistant.suggestion.spelling",
    "assistant.suggestion.next_scene",
];

/// 패널 폭.
const ASSISTANT_PANEL_WIDTH: f32 = 360.0;

impl HanjiApp {
    /// 어시스턴트 패널이 보이는지.
    pub(super) fn assistant_open(&self) -> bool {
        self.assistant_visible
    }

    /// 어시스턴트 패널 토글 버튼. 닫혀 있으면 에디터 툴바 오른쪽 끝에, 열려 있으면 패널 툴바
    /// 오른쪽 끝에 둔다(좁은 창에서 에디터 툴바가 잘려도 닫을 수 있게).
    pub(super) fn assistant_toggle(
        &self,
        cx: &mut Context<Self>,
        pal: Palette,
    ) -> impl IntoElement {
        tool_group(pal).child(
            icon_button("toggle-assistant", icons::assistant(), true, pal)
                .when(self.assistant_open(), |this| this.bg(pal.active))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.set_assistant_visible(!this.assistant_visible, cx);
                })),
        )
    }

    /// 오른쪽 어시스턴트 패널. 항상 그리고, 바깥 슬롯 폭만 스프링으로 0 ↔ 패널 폭 사이에서 움직인다.
    pub(super) fn render_assistant_panel(
        &self,
        cx: &mut Context<Self>,
        pal: Palette,
    ) -> impl IntoElement {
        let configured = ai_configured(cx);
        let live = &self.assistant_chat;

        let toolbar = window_drag_area(
            div()
                .h(px(TOOLBAR_HEIGHT))
                .flex_shrink_0()
                .pl_4()
                .pr_3()
                .flex()
                .items_center()
                .gap_2(),
        )
        // 기록 화면이면 대화로 돌아가는 버튼, 대화 화면이면 기록을 여는 버튼을 둔다.
        .child(
            tool_group(pal).child(
                icon_button(
                    "assistant-history",
                    if self.assistant_history {
                        icons::back()
                    } else {
                        icons::history()
                    },
                    true,
                    pal,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.assistant_history = !this.assistant_history;
                    if this.assistant_history {
                        this.load_assistant_history(window, cx);
                    }
                    cx.notify();
                })),
            ),
        )
        .child(div().flex_1().min_w_0().truncate().font_semibold().child(
            if self.assistant_history {
                SharedString::from(t!("assistant.history.title"))
            } else {
                live.conversation
                    .title()
                    .map(|title| SharedString::from(title.to_string()))
                    .unwrap_or_else(|| t!("assistant.new_conversation").into())
            },
        ))
        .child(tool_group(pal).child(
            icon_button("assistant-new", icons::compose(), true, pal).on_click(cx.listener(
                |this, _, window, cx| {
                    this.assistant_history = false;
                    this.reset_live_chat(window, cx);
                },
            )),
        ))
        .child(self.assistant_toggle(cx, pal));

        let messages: AnyElement = match () {
            () if live.conversation.is_empty() => self
                .render_assistant_empty(configured, cx, pal)
                .into_any_element(),
            () => {
                let turns = live.conversation.turns();
                let streaming = live.conversation.is_streaming();
                let last = turns.len().saturating_sub(1);
                div()
                    .id("assistant-live-messages")
                    .track_scroll(&live.scroll)
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_4()
                    .py_3()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .children(turns.iter().enumerate().map(|(ix, turn)| {
                        // 글이 오기 전의 빈 답은 받는 중 표시로 그린다.
                        let text = if !turn.user && turn.text.is_empty() && streaming && ix == last
                        {
                            "…".into()
                        } else {
                            turn.text.trim_end().to_string().into()
                        };
                        let id = ("assistant-live-message", ix);
                        if turn.user {
                            message_bubble(id, true, text, pal)
                        } else {
                            self.render_reply(id, text, &turn.activity, cx, pal)
                        }
                    }))
                    .children(live.conversation.error().map(|error| {
                        div()
                            .px_3()
                            .py_2()
                            .rounded_lg()
                            .border_1()
                            .border_color(pal.danger.opacity(0.4))
                            .text_sm()
                            .text_color(pal.danger)
                            .child(t!("assistant.error.no_reply", error = error))
                    }))
                    .into_any_element()
            }
        };

        // 입력창. AI를 설정하지 않았으면 두지 않는다.
        // 답을 받는 중에는 보내기 대신 멈추기 버튼을 둔다.
        let can_send = !live.conversation.is_streaming()
            && self.assistant_model_usable(cx)
            && !live.input.read(cx).value().trim().is_empty();
        let send_button = if live.conversation.is_streaming() {
            icon_button("assistant-stop", icons::stop(), true, pal)
                .on_click(cx.listener(|this, _, window, cx| this.stop_assistant_reply(window, cx)))
        } else {
            icon_button("assistant-send", icons::send(), can_send, pal).when(can_send, |this| {
                this.on_click(cx.listener(|this, _, window, cx| {
                    let text = this.assistant_chat.input.read(cx).value().to_string();
                    this.send_assistant_message(text, window, cx);
                }))
            })
        };
        let composer = div().flex_shrink_0().p_3().when(configured, |this| {
            this.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .pl_1()
                    .pr_2()
                    .py_1()
                    .rounded_lg()
                    .border_1()
                    .border_color(pal.panel_border)
                    .bg(pal.panel)
                    .child(
                        div()
                            .text_sm()
                            .child(Textarea::new(&live.input).appearance(false)),
                    )
                    // 아래 줄: 모델 고르기와 보내기(멈추기) 버튼.
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(self.render_model_picker(pal)),
                            )
                            .child(send_button),
                    ),
            )
        });

        let panel = div()
            .w(px(ASSISTANT_PANEL_WIDTH))
            .h_full()
            .flex()
            .flex_col()
            .bg(pal.background)
            .border_l_1()
            .border_color(pal.border)
            .child(toolbar)
            .map(|this| {
                if self.assistant_history {
                    this.child(self.render_assistant_history(cx, pal))
                } else {
                    this.child(messages).child(composer)
                }
            });

        div()
            .h_full()
            .flex_shrink_0()
            .overflow_hidden()
            .child(panel)
            .with_spring(
                "assistant-panel",
                SpringAnimation::new(SIDEBAR_SPRING).to(if self.assistant_open() {
                    1.0_f32
                } else {
                    0.0
                }),
                |this, open| this.w(px(ASSISTANT_PANEL_WIDTH * open.clamp(0.0, 1.0))),
            )
    }
}

impl HanjiApp {
    /// 대화 기록 화면. 기간(오늘·어제·지난 7일·이전)으로 묶고, 줄마다 제목·시각·마지막 말을 보인다.
    /// 줄을 누르면 그 대화를 다시 열어 이어 간다. 시트를 열었으면 그 시트의 대화만 볼 수 있다.
    fn render_assistant_history(&self, cx: &mut Context<Self>, pal: Palette) -> impl IntoElement {
        let live = &self.assistant_chat;
        let current = live.saved.as_ref().map(|saved| saved.summary.id);
        let sheet = self.open_sheet_path();
        let sheet_only = self.assistant_sheet_only && sheet.is_some();
        let today = Local::now().date_naive();

        let mut rows: Vec<AnyElement> = Vec::new();
        if sheet.is_some() {
            let chip = |id: &'static str, label: std::borrow::Cow<'static, str>, on: bool| {
                div()
                    .id(id)
                    .px_2()
                    .py(px(2.0))
                    .rounded_md()
                    .text_xs()
                    .cursor_pointer()
                    .when(on, |this| this.bg(pal.active).font_medium())
                    .when(!on, |this| {
                        this.text_color(pal.muted)
                            .hover(move |style| style.bg(pal.hover))
                    })
                    .child(label)
            };
            rows.push(
                div()
                    .px_2()
                    .pt_2()
                    .flex()
                    .gap_1()
                    .child(
                        chip(
                            "assistant-history-all",
                            t!("assistant.history.all"),
                            !sheet_only,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.assistant_sheet_only = false;
                            cx.notify();
                        })),
                    )
                    .child(
                        chip(
                            "assistant-history-sheet",
                            t!("assistant.history.this_sheet"),
                            sheet_only,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.assistant_sheet_only = true;
                            cx.notify();
                        })),
                    )
                    .into_any_element(),
            );
        }

        let shown: Vec<&agent::Summary> = live
            .summaries
            .iter()
            .filter(|summary| {
                !sheet_only
                    || sheet
                        .as_ref()
                        .is_some_and(|path| summary.sheets.contains(path))
            })
            .collect();
        if shown.is_empty() {
            rows.push(
                div()
                    .px_2()
                    .pt_6()
                    .text_sm()
                    .text_center()
                    .text_color(pal.muted)
                    .child(t!("assistant.history.empty"))
                    .into_any_element(),
            );
        }

        let mut last_period = None;
        for summary in shown {
            let at = local_time(summary.updated);
            let period = at.map_or(Period::Earlier, |at| Period::of(at, today));
            if last_period != Some(period) {
                last_period = Some(period);
                rows.push(
                    div()
                        .px_2()
                        .pt_3()
                        .pb_1()
                        .text_xs()
                        .font_semibold()
                        .text_color(pal.muted)
                        .child(period.label())
                        .into_any_element(),
                );
            }
            let id = summary.id;
            let selected = Some(id) == current;
            let row_id =
                ElementId::from(SharedString::from(format!("assistant-conversation-{id}")));
            let delete_id = ElementId::from(SharedString::from(format!("assistant-delete-{id}")));
            let group = SharedString::from(format!("assistant-row-{id}"));
            rows.push(
                div()
                    .id(row_id)
                    .group(group.clone())
                    .px_2()
                    .py_2()
                    .rounded_md()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .cursor_pointer()
                    .when(selected, |this| this.bg(pal.active))
                    .when(!selected, |this| {
                        this.hover(move |style| style.bg(pal.hover))
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_assistant_conversation(id, window, cx);
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_sm()
                                    .font_medium()
                                    .child(summary.title.clone()),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_xs()
                                    .text_color(pal.muted)
                                    .child(at.map(|at| time_label(at, today)).unwrap_or_default()),
                            )
                            // 지우기는 줄 위에 있을 때만 보인다.
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .invisible()
                                    .group_hover(group, |style| style.visible())
                                    .child(
                                        icon_button(delete_id, icons::trash(), true, pal).on_click(
                                            cx.listener(move |this, _, window, cx| {
                                                cx.stop_propagation();
                                                this.delete_assistant_conversation(id, window, cx);
                                            }),
                                        ),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(pal.muted)
                            .child(summary.preview.clone()),
                    )
                    .into_any_element(),
            );
        }

        div()
            .id("assistant-history")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_2()
            .pb_3()
            .flex()
            .flex_col()
            .gap(px(1.0))
            .children(rows)
    }
}

impl HanjiApp {
    /// 빈 대화. AI를 설정했으면 열린 시트를 두고 물을 수 있다는 안내와 질문 예시를, 아니면
    /// 설정 창을 여는 버튼을 보인다.
    fn render_assistant_empty(
        &self,
        configured: bool,
        cx: &mut Context<Self>,
        pal: Palette,
    ) -> impl IntoElement {
        // 시트를 열지 않았으면 라이브러리 전체를 두고 묻는다. 질문 예시는 시트를 두고 묻는 말이라 숨긴다.
        let about = match &self.editor {
            Some(editor) => {
                let title = self
                    .previews
                    .get(editor.path())
                    .map(|(_, preview)| preview.title.trim().to_string())
                    .filter(|title| !title.is_empty())
                    .unwrap_or_else(|| t!("assistant.untitled_sheet").to_string());
                t!("assistant.empty.about_sheet", title = title)
            }
            None => t!("assistant.empty.about_library"),
        };
        let suggestions = if self.editor.is_some() {
            SUGGESTIONS
        } else {
            &[]
        };

        let badge = div()
            .size(px(48.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(pal.tint)
            .child(
                Icon::new(icons::assistant())
                    .large()
                    .text_color(pal.tint_foreground),
            );

        let body = if configured {
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .text_lg()
                        .font_semibold()
                        .child(t!("assistant.empty.title")),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(pal.muted)
                        .text_center()
                        .child(about),
                )
                .child(div().w_full().mt_5().flex().flex_col().gap_2().children(
                    suggestions.iter().enumerate().map(|(ix, key)| {
                        let text = t!(*key).to_string();
                        div()
                            .id(("assistant-suggestion", ix))
                            .px_3()
                            .py_2()
                            .rounded_lg()
                            .border_1()
                            .border_color(pal.panel_border)
                            .text_sm()
                            .cursor_pointer()
                            .hover(move |style| style.bg(pal.hover))
                            .child(text.clone())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.send_assistant_message(text.clone(), window, cx);
                            }))
                    }),
                ))
        } else {
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .text_lg()
                        .font_semibold()
                        .child(t!("assistant.setup.title")),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(pal.muted)
                        .text_center()
                        .child(t!("assistant.setup.description")),
                )
                .child(
                    div()
                        .id("assistant-open-settings")
                        .mt_4()
                        .px_4()
                        .py_2()
                        .rounded_full()
                        .bg(pal.tint)
                        .text_color(pal.tint_foreground)
                        .text_sm()
                        .font_medium()
                        .cursor_pointer()
                        .on_click(|_, _, cx| crate::settings::open(cx))
                        .child(t!("assistant.setup.open_settings")),
                )
        };

        div()
            .id("assistant-empty")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_6()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_4()
            .child(badge)
            .child(body)
    }
}

impl HanjiApp {
    /// 새 대화에 사용자 말을 더하고 모델에 보낸다. 답은 받는 대로 마지막 어시스턴트 답에 붙인다.
    /// 답을 받는 중이거나 빈 말이면 아무것도 하지 않는다.
    pub(super) fn send_assistant_message(
        &mut self,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(backend) = agent::Backend::from_prefs(cx.global::<crate::settings::Prefs>())
        else {
            return;
        };
        if text.trim().is_empty() || self.assistant_chat.conversation.is_streaming() {
            return;
        }
        // 에이전트가 열린 시트를 고칠 수 있으므로 먼저 저장해 둔다. 저장하지 않은 변경이 남아 있으면
        // 에이전트가 쓴 내용을 다음 자동 저장이 덮어쓴다.
        if let Err(err) = self.save_open_sheet(cx) {
            self.notify(t!("assistant.error.save_sheet", err = err), window, cx);
            return;
        }
        let workspace = self
            .library
            .as_ref()
            .map(|library| agent::Workspace::new(&library.root));
        let system = self.assistant_system_prompt(workspace.as_ref(), cx);
        let sheet = self.open_sheet_path();
        let chat = &mut self.assistant_chat;
        let Some(turns) = chat.conversation.send(&text, sheet.as_deref()) else {
            return;
        };

        // 기록을 보고 있었어도 대화로 돌아와 보낸다.
        self.assistant_history = false;
        let chat = &mut self.assistant_chat;
        chat.input
            .update(cx, |input, cx| input.set_value("", window, cx));
        chat.scroll.scroll_to_bottom();

        let mut events = agent::stream(agent::ChatRequest {
            backend,
            workspace,
            system,
            turns,
            allowed_sites: chat.conversation.allowed_sites(),
        });
        chat.reply = Some(cx.spawn_in(window, async move |this, cx| {
            while let Some(event) = events.next().await {
                let finished =
                    matches!(event, agent::ChatEvent::Done | agent::ChatEvent::Failed(_));
                let updated = this.update_in(cx, |this, window, cx| {
                    this.on_assistant_event(event, window, cx);
                });
                if updated.is_err() || finished {
                    break;
                }
            }
        }));
        cx.notify();
    }

    /// AI 설정(제공자·프로필·리전)이 바뀌었으면 그 설정으로 쓸 수 있는 모델을 다시 확인한다.
    pub(super) fn refresh_assistant_models(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let backend = agent::Backend::from_prefs(cx.global::<crate::settings::Prefs>());
        let key = backend.as_ref().map(agent::Backend::models_key);
        let chat = &mut self.assistant_chat;
        if chat.models_for == key && !matches!(chat.models, ModelLookup::Failed(_)) {
            return;
        }
        chat.models_for = key.clone();
        chat.model_picker.update(cx, |picker, cx| {
            picker.set_items(SearchableVec::new(Vec::<ModelItem>::new()), window, cx);
        });
        let Some(backend) = backend else {
            chat.models = ModelLookup::Idle;
            chat.model_lookup = None;
            cx.notify();
            return;
        };
        chat.models = ModelLookup::Loading;
        let lookup = backend.available_models();
        chat.model_lookup = Some(cx.spawn_in(window, async move |this, cx| {
            let result = lookup
                .await
                .unwrap_or_else(|_| Err(t!("assistant.model.lookup_stopped").into()));
            this.update_in(cx, |this, window, cx| {
                this.apply_assistant_models(result, window, cx)
            })
            .ok();
        }));
        cx.notify();
    }

    /// 확인한 모델로 고르기 목록을 채운다. 저장한 모델이 이 리전에서는 다른 추론 프로필
    /// (`us.` ↔ `global.` 등)이면 그 ID로 바꿔 저장한다.
    fn apply_assistant_models(
        &mut self,
        result: Result<Vec<(agent::ModelChoice, String)>, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let models = match result {
            Ok(models) => models,
            Err(err) => {
                self.assistant_chat.models = ModelLookup::Failed(err.into());
                cx.notify();
                return;
            }
        };
        // 고른 적이 없으면 제공자의 기본 모델을 고른 것으로 보인다. 고른 모델(또는 기본)을 쓸 수 없으면
        // 목록의 첫 모델로 바꾼다.
        let wanted = agent::Backend::from_prefs(cx.global::<crate::settings::Prefs>())
            .map(|backend| backend.model().to_string())
            .unwrap_or_default();
        let selected = models
            .iter()
            .position(|(model, _)| model.matches(&wanted))
            .or_else(|| (!models.is_empty()).then_some(0));
        if let Some(ix) = selected
            && models[ix].1 != wanted
        {
            crate::settings::Prefs::set_ai_model(Some(models[ix].1.clone().into()), cx);
        }
        let items: Vec<_> = models
            .iter()
            .map(|(model, id)| ModelItem {
                title: model.name.to_string().into(),
                id: id.clone().into(),
            })
            .collect();
        let chat = &mut self.assistant_chat;
        chat.model_picker.update(cx, |picker, cx| {
            picker.set_items(SearchableVec::new(items), window, cx);
            picker.set_selected_index(selected.map(IndexPath::new), window, cx);
        });
        chat.models = ModelLookup::Ready(models);
        cx.notify();
    }

    /// 어시스턴트가 고른 모델(`set_model`)을 저장하고 모델 고르기에도 보인다.
    fn choose_assistant_model(&mut self, model: &str, window: &mut Window, cx: &mut Context<Self>) {
        crate::settings::Prefs::set_ai_model(Some(model.to_string().into()), cx);
        let chat = &mut self.assistant_chat;
        if let ModelLookup::Ready(models) = &chat.models {
            let selected = models.iter().position(|(_, id)| id == model);
            chat.model_picker.update(cx, |picker, cx| {
                picker.set_selected_index(selected.map(IndexPath::new), window, cx);
            });
        }
    }

    /// 지금 고른 모델로 보낼 수 있는지. 목록을 확인하지 못했으면 일단 보내 본다(실패하면 안내).
    fn assistant_model_usable(&self, cx: &App) -> bool {
        match &self.assistant_chat.models {
            ModelLookup::Ready(models) => agent::Backend::from_prefs(
                cx.global::<crate::settings::Prefs>(),
            )
            .is_some_and(|backend| {
                models
                    .iter()
                    .any(|(model, _)| model.matches(backend.model()))
            }),
            ModelLookup::Loading => false,
            ModelLookup::Idle | ModelLookup::Failed(_) => true,
        }
    }

    /// 입력창 아래 모델 고르기. 확인 중·실패·없음이면 고르기 대신 짧은 안내를 둔다.
    fn render_model_picker(&self, pal: Palette) -> AnyElement {
        let note = |text: std::borrow::Cow<'static, str>, color: Hsla| {
            div()
                .px_2()
                .text_xs()
                .truncate()
                .text_color(color)
                .child(text)
                .into_any_element()
        };
        match &self.assistant_chat.models {
            ModelLookup::Idle => note("".into(), pal.muted),
            ModelLookup::Loading => note(t!("assistant.model.loading"), pal.muted),
            ModelLookup::Failed(err) => div()
                .id("assistant-model-failed")
                .px_2()
                .text_xs()
                .truncate()
                .text_color(pal.danger)
                .tooltip({
                    let err = err.clone();
                    move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(err.clone()).build(window, cx)
                    }
                })
                .child(t!("assistant.model.failed"))
                .into_any_element(),
            ModelLookup::Ready(models) if models.is_empty() => {
                note(t!("assistant.model.none_in_region"), pal.danger)
            }
            ModelLookup::Ready(_) => Select::new(&self.assistant_chat.model_picker)
                .small()
                .appearance(false)
                .focus_ring(false)
                .placeholder(t!("assistant.model.placeholder"))
                .menu_width(px(220.0))
                .into_any_element(),
        }
    }

    /// 받은 일을 대화에 반영한다. 답이 한 글자도 오지 않고 실패했으면 보낸 말을 입력창으로
    /// 되돌려 다시 보낼 수 있게 한다.
    fn on_assistant_event(
        &mut self,
        event: agent::ChatEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let finished = matches!(event, agent::ChatEvent::Done | agent::ChatEvent::Failed(_));
        match &event {
            agent::ChatEvent::SetModel(model) => self.choose_assistant_model(model, window, cx),
            agent::ChatEvent::Changed => self.sync_open_sheet(window, cx),
            _ => {}
        }
        let chat = &mut self.assistant_chat;
        if let Some(sent) = chat.conversation.apply(event) {
            chat.input
                .update(cx, |input, cx| input.set_value(sent, window, cx));
        }
        chat.scroll.scroll_to_bottom();
        if finished {
            self.save_assistant_chat(window, cx);
        }
        cx.notify();
    }

    /// 받는 중인 답을 멈춘다. 받은 데까지는 남기고 기록에 쓴다.
    fn stop_assistant_reply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chat = &mut self.assistant_chat;
        chat.reply = None;
        chat.conversation.stop();
        self.save_assistant_chat(window, cx);
        cx.notify();
    }

    /// 새 대화로 비운다. 받는 중인 답은 멈추고 받은 데까지 기록에 쓴다.
    fn reset_live_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.assistant_chat.conversation.is_streaming() {
            self.stop_assistant_reply(window, cx);
        }
        let chat = &mut self.assistant_chat;
        chat.reply = None;
        chat.conversation.clear();
        chat.saved = None;
        chat.change_scrolls.get_mut().clear();
        cx.notify();
    }

    /// 이 라이브러리의 대화 기록. 처음 부를 때(라이브러리가 바뀌었으면 다시) 연다. 열지 못하면 한 번
    /// 알리고 기록 없이 쓴다.
    fn assistant_store(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Arc<agent::History>> {
        let root = self.library.as_ref()?.root.clone();
        if self
            .assistant_store
            .as_ref()
            .is_none_or(|(opened, _)| *opened != root)
        {
            // 다른 라이브러리의 대화를 이어 가면 이 라이브러리에는 새 대화로 쓴다.
            if self.assistant_store.is_some() {
                self.assistant_chat.saved = None;
                self.assistant_chat.summaries.clear();
            }
            let store = match agent::History::open(&root) {
                Ok(store) => Some(Arc::new(store)),
                Err(err) => {
                    self.notify(t!("assistant.error.history_open", err = err), window, cx);
                    None
                }
            };
            self.assistant_store = Some((root, store));
        }
        self.assistant_store.as_ref()?.1.clone()
    }

    /// 기록 화면에 보일 요약을 읽는다.
    fn load_assistant_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let summaries = self
            .assistant_store(window, cx)
            .map(|store| store.list())
            .unwrap_or_default();
        self.assistant_chat.summaries = summaries;
    }

    /// 지금 대화를 기록에 쓴다. 지난번에 쓴 뒤의 말만 백그라운드에서 쓴다.
    fn save_assistant_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.assistant_chat.conversation.turns().is_empty() {
            return;
        }
        let Some(store) = self.assistant_store(window, cx) else {
            return;
        };
        let chat = &mut self.assistant_chat;
        let turns = chat.conversation.turns().to_vec();
        let now = agent::now_millis();
        let saved = chat.saved.get_or_insert_with(|| Saved {
            summary: agent::Summary {
                id: store.next_id(),
                created: now,
                ..agent::Summary::default()
            },
            persisted: 0,
        });
        saved.summary.updated = now;
        saved.summary.describe(&turns);
        saved.summary.sites = chat.conversation.sites();
        let from = saved.persisted.min(turns.len());
        saved.persisted = turns.len();
        let summary = saved.summary.clone();
        chat.summaries.retain(|item| item.id != summary.id);
        chat.summaries.insert(0, summary.clone());

        let previous = chat.save_task.take();
        chat.save_task = Some(cx.spawn_in(window, async move |this, cx| {
            if let Some(previous) = previous {
                previous.await;
            }
            let id = summary.id;
            let result = cx
                .background_executor()
                .spawn(async move { store.save(&summary, &turns, from) })
                .await;
            if let Err(err) = result {
                this.update_in(cx, |this, window, cx| {
                    // 다음에 대화 전체를 다시 쓴다.
                    if let Some(saved) = &mut this.assistant_chat.saved
                        && saved.summary.id == id
                    {
                        saved.persisted = 0;
                    }
                    this.notify(t!("assistant.error.history_save", err = err), window, cx);
                })
                .ok();
            }
        }));
    }

    /// 기록의 대화를 다시 연다. 받는 중인 답은 멈추고 기록에 쓴다.
    fn open_assistant_conversation(
        &mut self,
        id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(store) = self.assistant_store(window, cx) else {
            return;
        };
        let Some(summary) = self
            .assistant_chat
            .summaries
            .iter()
            .find(|summary| summary.id == id)
            .cloned()
        else {
            return;
        };
        let Some(turns) = store.load(id) else {
            self.notify(t!("assistant.error.history_missing"), window, cx);
            return;
        };
        self.reset_live_chat(window, cx);
        let chat = &mut self.assistant_chat;
        chat.saved = Some(Saved {
            summary: summary.clone(),
            persisted: turns.len(),
        });
        chat.conversation = agent::Conversation::restore(turns, summary.sites);
        chat.scroll.scroll_to_bottom();
        self.assistant_history = false;
        cx.notify();
    }

    /// 기록에서 대화를 지운다. 보고 있던 대화면 새 대화로 비운다.
    fn delete_assistant_conversation(
        &mut self,
        id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(store) = self.assistant_store(window, cx) else {
            return;
        };
        if let Err(err) = store.delete(id) {
            self.notify(t!("assistant.error.history_delete", err = err), window, cx);
            return;
        }
        let current = self
            .assistant_chat
            .saved
            .as_ref()
            .map(|saved| saved.summary.id);
        if current == Some(id) {
            self.reset_live_chat(window, cx);
        }
        self.assistant_chat
            .summaries
            .retain(|summary| summary.id != id);
        cx.notify();
    }

    /// 열린 시트의 라이브러리 루트 기준 경로(파일 도구가 쓰는 경로).
    fn open_sheet_path(&self) -> Option<String> {
        let editor = self.editor.as_ref()?;
        let relative = editor
            .path()
            .strip_prefix(&self.library.as_ref()?.root)
            .ok()?;
        Some(relative.to_string_lossy().into_owned())
    }

    /// 열린 시트를 넣은 시스템 프롬프트.
    fn assistant_system_prompt(&self, workspace: Option<&agent::Workspace>, cx: &App) -> String {
        let tools = workspace.is_some();
        let skills = workspace.map_or(&[][..], agent::Workspace::skills);
        let extensions = workspace.map_or(&[][..], agent::Workspace::extensions);
        let Some(editor) = &self.editor else {
            return agent::system_prompt(None, tools, skills, extensions);
        };
        let path = self.open_sheet_path();
        let title = self
            .previews
            .get(editor.path())
            .map(|(_, preview)| preview.title.to_string())
            .unwrap_or_default();
        // 파일과 같은 마크다운으로 넣는다(에디터 본문은 줄 하나가 문단이라 파일과 다르다). 그래야
        // edit_file의 old를 파일에서 그대로 찾는다.
        let body = crate::text_transform::to_markdown(&editor.state().read(cx).value());
        agent::system_prompt(
            Some(agent::Sheet {
                title: &title,
                path: path.as_deref(),
                body: &body,
            }),
            tools,
            skills,
            extensions,
        )
    }
}

impl HanjiApp {
    /// 어시스턴트 답: 한 일(도구 줄, 승인 카드)과 글.
    fn render_reply(
        &self,
        id: impl Into<ElementId>,
        text: SharedString,
        activity: &[agent::Activity],
        cx: &mut Context<Self>,
        pal: Palette,
    ) -> AnyElement {
        let pending = self.assistant_chat.conversation.pending_change();
        let activity = activity
            .iter()
            .map(|activity| match activity {
                agent::Activity::Tool(summary) => div()
                    .text_xs()
                    .truncate()
                    .text_color(pal.muted)
                    .child(SharedString::from(summary.clone()))
                    .into_any_element(),
                agent::Activity::Review { file, score } => review_line(file, *score, pal),
                agent::Activity::Change(card) => {
                    let scroll = self
                        .assistant_chat
                        .change_scrolls
                        .borrow_mut()
                        .entry(card.id)
                        .or_default()
                        .clone();
                    change_card(card, pending == Some(card.id), &scroll, cx, pal)
                }
                agent::Activity::Fetch(card) => fetch_card(card, pending == Some(card.id), cx, pal),
            })
            .collect();
        assistant_reply(id, false, text, activity, pal)
    }
}

/// 검토 줄: "검토 · 파일"과 점수(10점을 별 다섯 개로, 반 별까지). 통과 점수부터 성공 색, 그 아래는 옅은 색,
/// 3점 이하는 위험 색이다. 숫자는 별 뒤에 작게 둔다.
fn review_line(file: &str, score: Option<u8>, pal: Palette) -> AnyElement {
    let line = div()
        .flex()
        .items_center()
        .gap_1()
        .text_xs()
        .text_color(pal.muted)
        .child(
            div()
                .truncate()
                .child(format!("{} · {file}", t!("assistant.tool.reviewed"))),
        );
    let Some(score) = score else {
        return line.into_any_element();
    };
    let color = if score >= crate::agent::PASS_SCORE {
        pal.success
    } else if score <= 3 {
        pal.danger
    } else {
        pal.muted
    };
    let stars = (0..5u8).map(|ix| {
        let icon = match score.saturating_sub(ix * 2) {
            0 => icons::star_empty(),
            1 => icons::star_half(),
            _ => icons::star_full(),
        };
        Icon::new(icon).xsmall().text_color(color)
    });
    line.child(div().flex().flex_none().children(stars))
        .child(div().flex_none().child(format!("{score}/10")))
        .into_any_element()
}

/// 승인 카드: 바꿀 파일, 변경 내용(diff), 그리고 기다리는 중이면 [취소] [적용] 버튼.
fn change_card(
    card: &agent::ChangeCard,
    waiting: bool,
    scroll: &ScrollHandle,
    cx: &mut Context<HanjiApp>,
    pal: Palette,
) -> AnyElement {
    let id = card.id as usize;
    let status = match card.state {
        agent::ChangeState::Pending if card.new_file => t!("assistant.change.pending_new"),
        agent::ChangeState::Pending => t!("assistant.change.pending_edit"),
        agent::ChangeState::Applied => t!("assistant.change.applied"),
        agent::ChangeState::Declined => t!("assistant.change.declined"),
        agent::ChangeState::Failed => t!("assistant.change.failed"),
    };
    let status_color = match card.state {
        agent::ChangeState::Applied => pal.success,
        agent::ChangeState::Failed => pal.danger,
        _ => pal.muted,
    };
    let card_id = card.id;
    let decide = |approve: bool| {
        cx.listener(move |this: &mut HanjiApp, _: &ClickEvent, window, cx| {
            // 보낸 뒤 에디터에서 고친 글을 먼저 저장한다. 그래야 모델이 읽은 뒤 시트가 바뀌었으면
            // 쓰기가 멈추고, 저장하지 않은 변경이 모델이 쓴 내용을 덮어쓰지 않는다.
            if approve && let Err(err) = this.save_open_sheet(cx) {
                this.notify(t!("assistant.error.save_sheet", err = err), window, cx);
                return;
            }
            this.assistant_chat.conversation.decide(card_id, approve);
            cx.notify();
        })
    };
    let (decline, apply) = (decide(false), decide(true));
    div()
        .w_full()
        .rounded_lg()
        .border_1()
        .border_color(pal.panel_border)
        .overflow_hidden()
        .child(
            div()
                .px_2()
                .py_1()
                .flex()
                .items_center()
                .gap_2()
                .text_xs()
                .bg(pal.hover)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(SharedString::from(card.path.clone())),
                )
                .child(div().flex_shrink_0().text_color(status_color).child(status)),
        )
        .child(
            div()
                .id(("change-lines", id))
                .track_scroll(scroll)
                .max_h(px(260.0))
                .overflow_y_scroll()
                // 바깥 대화 목록도 같은 휠을 받으므로, diff가 넘쳐 스스로 스크롤할 때는 넘기지 않는다.
                .on_scroll_wheel({
                    let scroll = scroll.clone();
                    move |_, _, cx| {
                        if scroll.max_offset().y > px(0.0) {
                            cx.stop_propagation();
                        }
                    }
                })
                .py_1()
                .flex()
                .flex_col()
                .children(card.lines.iter().map(|line| diff_line(line, pal))),
        )
        .when(waiting, |this| {
            this.child(
                div()
                    .px_2()
                    .py_1()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .border_t_1()
                    .border_color(pal.panel_border)
                    .child(
                        Button::new(("change-decline", id))
                            .small()
                            .label(t!("common.cancel"))
                            .on_click(decline),
                    )
                    .child(
                        Button::new(("change-apply", id))
                            .small()
                            .primary()
                            .label(t!("assistant.change.apply"))
                            .on_click(apply),
                    ),
            )
        })
        .into_any_element()
}

/// 웹 페이지 읽기 허락 카드: 읽을 주소 전체와, 기다리는 중이면 [거절] [허용] 버튼.
/// 주소를 다 보여 주는 것은 모델이 주소에 글을 실어 내보내지 않는지 사용자가 볼 수 있게 하려는 것이다.
fn fetch_card(
    card: &agent::FetchCard,
    waiting: bool,
    cx: &mut Context<HanjiApp>,
    pal: Palette,
) -> AnyElement {
    let id = card.id as usize;
    let (status, status_color) = match card.state {
        agent::ChangeState::Pending => (t!("assistant.fetch.pending"), pal.muted),
        agent::ChangeState::Applied => (t!("assistant.fetch.allowed"), pal.success),
        agent::ChangeState::Declined | agent::ChangeState::Failed => {
            (t!("assistant.fetch.declined"), pal.muted)
        }
    };
    let card_id = card.id;
    let decide = |approve: bool| {
        cx.listener(move |this: &mut HanjiApp, _: &ClickEvent, _, cx| {
            this.assistant_chat.conversation.decide(card_id, approve);
            cx.notify();
        })
    };
    let (decline, allow) = (decide(false), decide(true));
    div()
        .w_full()
        .rounded_lg()
        .border_1()
        .border_color(pal.panel_border)
        .overflow_hidden()
        .child(
            div()
                .px_2()
                .py_1()
                .flex()
                .items_center()
                .gap_2()
                .text_xs()
                .bg(pal.hover)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(t!("assistant.fetch.title", site = card.site.clone())),
                )
                .child(div().flex_shrink_0().text_color(status_color).child(status)),
        )
        .child(
            div()
                .px_2()
                .py_1()
                .text_xs()
                .font_family("monospace")
                .text_color(pal.foreground)
                .child(SharedString::from(card.url.clone())),
        )
        .when(waiting, |this| {
            this.child(
                div()
                    .px_2()
                    .py_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_t_1()
                    .border_color(pal.panel_border)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .text_color(pal.muted)
                            .child(t!("assistant.fetch.note", site = card.site.clone())),
                    )
                    .child(
                        Button::new(("fetch-decline", id))
                            .small()
                            .label(t!("assistant.fetch.decline"))
                            .on_click(decline),
                    )
                    .child(
                        Button::new(("fetch-allow", id))
                            .small()
                            .primary()
                            .label(t!("assistant.fetch.allow"))
                            .on_click(allow),
                    ),
            )
        })
        .into_any_element()
}

/// diff 한 줄. 지운 줄은 빨강, 더한 줄은 초록 바탕이고, 줄 안에서 바뀐 글자는 더 진하게 칠한다.
fn diff_line(line: &agent::DiffLine, pal: Palette) -> AnyElement {
    let (sign, color) = match line.kind {
        agent::LineKind::Removed => ("−", Some(pal.danger)),
        agent::LineKind::Added => ("+", Some(pal.success)),
        agent::LineKind::Context => (" ", None),
        agent::LineKind::Gap => {
            return div()
                .px_2()
                .text_xs()
                .text_color(pal.muted)
                .child("⋯")
                .into_any_element();
        }
        // 파일이 여럿인 변경(폴더 확장)의 파일 머리줄.
        agent::LineKind::File => {
            return div()
                .px_2()
                .pt_1()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(pal.foreground)
                .border_t_1()
                .border_color(pal.border)
                .child(SharedString::from(line.text.clone()))
                .into_any_element();
        }
    };
    // 빈 줄도 한 줄 높이를 차지하게 한다.
    let text: SharedString = if line.text.is_empty() {
        " ".into()
    } else {
        line.text.clone().into()
    };
    let highlights: Vec<_> = match color {
        Some(color) if !line.text.is_empty() => line
            .emphasis
            .iter()
            .map(|range| {
                (
                    range.clone(),
                    HighlightStyle {
                        background_color: Some(color.opacity(0.35)),
                        ..Default::default()
                    },
                )
            })
            .collect(),
        _ => Vec::new(),
    };
    div()
        .px_2()
        .flex()
        .gap_1()
        .text_xs()
        .line_height(relative(1.5))
        .when_some(color, |this, color| this.bg(color.opacity(0.1)))
        .when(color.is_none(), |this| this.text_color(pal.muted))
        .child(div().flex_shrink_0().w(px(10.0)).child(sign))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(StyledText::new(text).with_highlights(highlights)),
        )
        .into_any_element()
}

/// AI를 쓸 준비가 됐는지: 제공자를 골랐고, Bedrock이면 AWS 프로필, Token Plan이면 API 키가 있다.
fn ai_configured(cx: &App) -> bool {
    agent::Backend::from_prefs(cx.global::<crate::settings::Prefs>()).is_some()
}

/// 대화 한 줄. 내 말은 오른쪽 말풍선, 어시스턴트 말은 왼쪽 본문으로 그린다.
/// 말풍선. 어시스턴트 답이면 한 일(`activity`: 부른 도구, 승인 카드)을 글 위에 둔다.
fn message_bubble(
    id: impl Into<ElementId>,
    user: bool,
    text: SharedString,
    pal: Palette,
) -> AnyElement {
    assistant_reply(id, user, text, Vec::new(), pal)
}

/// 말 하나. 어시스턴트 답은 마크다운으로 그리고, 드래그해 고른 부분을 복사할 수 있다(한 서식 안이면 글자만,
/// 여러 서식에 걸치면 마크다운 원문).
fn assistant_reply(
    id: impl Into<ElementId>,
    user: bool,
    text: SharedString,
    activity: Vec<AnyElement>,
    pal: Palette,
) -> AnyElement {
    if user {
        div()
            .flex()
            .justify_end()
            .child(
                div()
                    .max_w(px(ASSISTANT_PANEL_WIDTH * 0.8))
                    .px_3()
                    .py_2()
                    .rounded_lg()
                    .bg(pal.tint)
                    .text_sm()
                    .child(text),
            )
            .into_any_element()
    } else {
        div()
            .flex()
            .gap_2()
            .child(
                div()
                    .flex_shrink_0()
                    .pt(px(2.0))
                    .child(Icon::new(icons::assistant()).small().text_color(pal.muted)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children(activity)
                    .when(!text.is_empty(), |this| {
                        this.child(
                            div().text_sm().line_height(relative(1.5)).child(
                                text::TextView::markdown(id, text)
                                    .selectable(true)
                                    .selection_format(text::SelectionFormat::SourceAcrossStyles),
                            ),
                        )
                    }),
            )
            .into_any_element()
    }
}
