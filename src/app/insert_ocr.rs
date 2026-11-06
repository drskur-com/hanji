//! OCR 텍스트 삽입 대화상자(ADR-0053): 이미지를 고르면 글자를 읽어 미리 보여 주고, 삽입하면 열린 시트의
//! 커서 자리에 그 글을 넣는다. 미리보기는 고칠 수 있는 입력 칸이라 잘못 읽은 글자를 넣기 전에 바로잡는다.
//!
//! 먼저 운영체제의 인식기(`crate::ocr`: macOS Vision, Windows.Media.Ocr)가 백그라운드에서 읽는다. "AI로 읽기"를 켜면 이미지를 설정의 AI 제공자
//! 모델(LLM 비전)에 보내 받아 적게 하고(`agent::transcribe`), Vision 결과는 초안으로 함께 보낼 수 있다.
//! 한자를 섞은 글처럼 Vision이 못 읽는 글에 쓴다. 옵션(AI 사용·모델·초안·추가 지시·줄바꿈 유지)은
//! `config.toml`의 `[ocr]`에 저장해 다음에도 쓴다. 다른 이미지를 고르거나 다시 읽으면 하던 일은 버린다.

use futures::StreamExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::input::{EditorState, Textarea, TextareaState};
use gpui_kit::component::searchable_list::SearchableVec;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IndexPath, Sizable as _, WindowExt as _,
};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::HanjiApp;
use super::assistant_panel::ModelItem;
use super::new_project::form_row;
use super::style::*;
use crate::agent::{self, Backend, ChatEvent, transcribe};
use crate::config::{Config, OcrConfig};
use crate::ocr::{self, Line};
use crate::{InsertOcrText, icons, library};

/// 대화상자 폭.
const DIALOG_WIDTH: f32 = 560.0;

/// 이미지를 놓는 자리의 높이.
const SOURCE_HEIGHT: f32 = 150.0;

/// AI에 보내는 이미지의 긴 변(px). 작은 글자도 읽히고, 모델의 이미지 한도 안에 든다.
const AI_IMAGE_SIDE: u32 = 2048;

/// AI에 보내는 PNG가 이보다 크면 JPEG로 줄인다(Bedrock의 이미지 한도는 3.75 MB).
const AI_PNG_LIMIT: usize = 3 * 1024 * 1024;

/// 읽기 상태.
enum Status {
    /// 아직 이미지를 고르지 않았다.
    Idle,
    Recognizing,
    /// AI가 받아 적는 중(모델 이름).
    Transcribing(SharedString),
    /// 기기 인식으로 다 읽었다(읽은 줄 수).
    Recognized(usize),
    /// AI가 다 받아 적었다.
    Transcribed,
    /// 실패했다. 미리보기에 기기 인식 결과가 있으면 그대로 둔다.
    Failed(SharedString),
}

impl Status {
    fn busy(&self) -> bool {
        matches!(self, Status::Recognizing | Status::Transcribing(_))
    }
}

/// AI로 읽을 수 있는 모델 확인 상태.
enum Models {
    /// AI를 설정하지 않았다.
    Unavailable,
    Loading,
    /// 그림을 볼 수 있는 모델과 부를 ID.
    Ready(Vec<(agent::ModelChoice, String)>),
    Failed(SharedString),
}

/// 백그라운드에서 준비한 것: 기기 인식 결과와 AI에 보낼 이미지(바이트, JPEG인지).
struct Prepared {
    lines: Result<Vec<Line>, String>,
    image: Option<(Vec<u8>, bool)>,
}

struct InsertOcrForm {
    file: Option<PathBuf>,
    file_modified: u128,
    status: Status,
    /// 마지막으로 읽은 줄. 줄바꿈 유지를 바꾸면 이것으로 미리보기를 다시 만든다.
    lines: Arc<Vec<Line>>,
    /// 미리보기가 AI의 답인지. 그러면 줄바꿈 유지를 바꿔도 다시 만들지 않는다(다시 읽기로 반영).
    from_ai: bool,
    /// 저장하는 옵션. 추가 지시는 입력 칸에 있고 저장할 때 옮긴다.
    options: OcrConfig,
    /// 설정의 AI 제공자. 설정하지 않았으면 `None`.
    backend: Option<Backend>,
    models: Models,
    model_picker: Entity<SelectState<SearchableVec<ModelItem>>>,
    /// AI로 읽을 모델 ID(목록에서 고른 것).
    model: Option<String>,
    instruction: Entity<TextareaState>,
    preview: Entity<TextareaState>,
    /// 진행 중인 읽기. 바꾸면 이전 작업(과 AI 요청)은 버려진다.
    _work: Option<Task<()>>,
    _model_lookup: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl InsertOcrForm {
    fn new(backend: Option<Backend>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let options = Config::load().ocr;
        let preview = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(5, 12)
                .placeholder(t!("insert.ocr.preview_placeholder"))
        });
        let instruction = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 5)
                .placeholder(t!("insert.ocr.instruction_placeholder"))
                .default_value(options.instruction.clone())
        });
        let model_picker = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(Vec::<ModelItem>::new()),
                None,
                window,
                cx,
            )
        });
        let subscription = cx.subscribe(
            &model_picker,
            |this, _, event: &SelectEvent<SearchableVec<ModelItem>>, cx| {
                let SelectEvent::Confirm(Some(id)) = event else {
                    return;
                };
                this.model = Some(id.to_string());
                *this.saved_model() = Some(id.to_string());
                this.save_options(cx);
            },
        );
        let mut form = Self {
            file: None,
            file_modified: 0,
            status: Status::Idle,
            lines: Arc::default(),
            from_ai: false,
            options,
            backend,
            models: Models::Unavailable,
            model_picker,
            model: None,
            instruction,
            preview,
            _work: None,
            _model_lookup: None,
            _subscriptions: vec![subscription],
        };
        form.load_models(window, cx);
        form
    }

    /// 지금 제공자에 저장한 OCR 모델 자리.
    fn saved_model(&mut self) -> &mut Option<String> {
        match self.backend {
            Some(Backend::TokenPlan { .. }) => &mut self.options.token_plan_model,
            _ => &mut self.options.bedrock_model,
        }
    }

    /// 옵션을 `config.toml`에 저장한다.
    fn save_options(&mut self, cx: &App) {
        self.options.instruction = self.instruction.read(cx).value().trim().to_string();
        let mut config = Config::load();
        config.ocr = self.options.clone();
        if let Err(err) = config.save() {
            eprintln!("설정 저장 실패: {err}");
        }
    }

    /// 제공자에서 그림을 볼 수 있는 모델을 확인해 모델 고르기를 채운다.
    fn load_models(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(backend) = self.backend.clone() else {
            return;
        };
        self.models = Models::Loading;
        let lookup = backend.available_models();
        self._model_lookup = Some(cx.spawn_in(window, async move |this, cx| {
            let result = lookup
                .await
                .unwrap_or_else(|_| Err(t!("assistant.model.lookup_stopped").into()));
            this.update_in(cx, |this, window, cx| this.apply_models(result, window, cx))
                .ok();
        }));
    }

    /// 확인한 모델로 고르기를 채운다. 저장한 모델(없으면 어시스턴트의 모델)을 쓸 수 없으면 첫 모델을 고른다.
    fn apply_models(
        &mut self,
        result: Result<Vec<(agent::ModelChoice, String)>, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(backend) = self.backend.clone() else {
            return;
        };
        let models: Vec<_> = match result {
            Ok(models) => models
                .into_iter()
                .filter(|(_, id)| backend.reads_images(id))
                .collect(),
            Err(err) => {
                self.models = Models::Failed(err.into());
                cx.notify();
                return;
            }
        };
        let wanted = self
            .saved_model()
            .clone()
            .unwrap_or_else(|| backend.model().to_string());
        let selected = models
            .iter()
            .position(|(model, _)| model.matches(&wanted))
            .or_else(|| (!models.is_empty()).then_some(0));
        self.model = selected.map(|ix| models[ix].1.clone());
        let items: Vec<_> = models
            .iter()
            .map(|(model, id)| ModelItem {
                title: model.name.to_string().into(),
                id: id.clone().into(),
            })
            .collect();
        self.model_picker.update(cx, |picker, cx| {
            picker.set_items(SearchableVec::new(items), window, cx);
            picker.set_selected_index(selected.map(IndexPath::new), window, cx);
        });
        self.models = Models::Ready(models);
        cx.notify();
    }

    /// AI로 읽을 수 있으면 부를 곳(고른 모델).
    fn ai_backend(&self) -> Option<Backend> {
        if !self.options.use_ai {
            return None;
        }
        let backend = self.backend.clone()?;
        Some(match &self.model {
            Some(model) => backend.with_model(model.clone()),
            None => backend,
        })
    }

    /// 고른 모델의 표시 이름.
    fn model_name(&self, backend: &Backend) -> SharedString {
        let id = backend.model();
        match &self.models {
            Models::Ready(models) => models
                .iter()
                .find(|(_, model)| model == id)
                .map_or_else(|| id.to_string(), |(choice, _)| choice.name.to_string()),
            _ => id.to_string(),
        }
        .into()
    }

    /// 운영체제 파일 선택기로 이미지를 고른다.
    fn pick_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t!("insert.ocr.choose_prompt").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = receiver.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| this.use_file(&paths, window, cx))
                .ok();
        })
        .detach();
    }

    /// 고른(끌어다 놓은) 파일 중 첫 이미지를 읽기 시작한다.
    fn use_file(&mut self, paths: &[PathBuf], window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = paths.iter().find(|path| ocr::is_image(path)).cloned() else {
            self.status = Status::Failed(t!("insert.ocr.unsupported").into());
            cx.notify();
            return;
        };
        self.file_modified = library::modified_nanos(&path);
        self.file = Some(path);
        self.read(window, cx);
    }

    /// 고른 이미지를 (다시) 읽는다. 기기 인식을 먼저 하고, AI로 읽기가 켜져 있으면 이어서 AI에 보낸다.
    fn read(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.file.clone() else {
            return;
        };
        self.save_options(cx);
        let ai = self.ai_backend();
        self.status = Status::Recognizing;
        self.lines = Arc::default();
        self.from_ai = false;
        self.preview
            .update(cx, |state, cx| state.set_value("", window, cx));
        let with_image = ai.is_some();
        let task = cx.background_executor().spawn(async move {
            Prepared {
                lines: ocr::recognize(&path),
                image: with_image.then(|| image_for_ai(&path)).flatten(),
            }
        });
        self._work = Some(cx.spawn_in(window, async move |this, cx| {
            let prepared = task.await;
            let Ok(Some(receiver)) = this.update_in(cx, |this, window, cx| {
                this.recognized(prepared, ai, window, cx)
            }) else {
                return;
            };
            let mut receiver = receiver;
            let mut answer = String::new();
            while let Some(event) = receiver.next().await {
                let updated = this.update_in(cx, |this, window, cx| {
                    this.on_transcribe_event(event, &mut answer, window, cx)
                });
                if !matches!(updated, Ok(true)) {
                    break;
                }
            }
        }));
        cx.notify();
    }

    /// 기기 인식이 끝났다. AI로 읽지 않으면 미리보기에 결과를 둔다. AI로 읽으면 결과는 초안·실패 때의 대체로만
    /// 두고(미리보기는 AI의 답을 기다린다), 요청을 보내 답을 받을 채널을 돌려준다.
    fn recognized(
        &mut self,
        prepared: Prepared,
        ai: Option<Backend>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<futures::channel::mpsc::UnboundedReceiver<ChatEvent>> {
        let recognized = match prepared.lines {
            Ok(lines) => {
                self.lines = Arc::new(lines);
                Ok(self.lines.len())
            }
            Err(err) => Err(err),
        };
        cx.notify();
        let Some(backend) = ai else {
            self.refresh_preview(window, cx);
            self.status = match recognized {
                Ok(count) => Status::Recognized(count),
                Err(err) => Status::Failed(t!("insert.ocr.failed", err = err).into()),
            };
            return None;
        };
        let Some((image, jpeg)) = prepared.image else {
            self.refresh_preview(window, cx);
            self.status = Status::Failed(t!("insert.ocr.image_failed").into());
            return None;
        };
        let draft = self
            .options
            .send_draft
            .then(|| ocr::join(&self.lines, self.options.keep_breaks));
        self.status = Status::Transcribing(self.model_name(&backend));
        Some(transcribe::transcribe(transcribe::TranscribeRequest {
            backend,
            image,
            jpeg,
            draft,
            instruction: self.options.instruction.clone(),
            keep_breaks: self.options.keep_breaks,
        }))
    }

    /// AI 답 조각을 미리보기에 반영한다. 더 받을 것이 있으면 `true`.
    fn on_transcribe_event(
        &mut self,
        event: ChatEvent,
        answer: &mut String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let more = match event {
            ChatEvent::Text(text) => {
                answer.push_str(&text);
                self.from_ai = true;
                let shown = answer.clone();
                self.preview
                    .update(cx, |state, cx| state.set_value(shown, window, cx));
                true
            }
            ChatEvent::Done => {
                let text = transcribe::clean(answer);
                self.from_ai = true;
                self.preview
                    .update(cx, |state, cx| state.set_value(text, window, cx));
                self.status = Status::Transcribed;
                false
            }
            ChatEvent::Failed(err) => {
                // 받은 데까지는 버리고 기기 인식 결과로 되돌린다.
                self.from_ai = false;
                self.refresh_preview(window, cx);
                self.status = Status::Failed(t!("insert.ocr.ai_failed", err = err).into());
                false
            }
            _ => true,
        };
        cx.notify();
        more
    }

    /// 기기 인식 결과로 미리보기를 다시 채운다. 고친 내용은 사라진다.
    fn refresh_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = ocr::join(&self.lines, self.options.keep_breaks);
        self.preview
            .update(cx, |state, cx| state.set_value(text, window, cx));
    }

    /// 넣을 글. 비었으면 `None`.
    fn text(&self, cx: &App) -> Option<String> {
        let text = self.preview.read(cx).value().trim().to_string();
        (!text.is_empty()).then_some(text)
    }

    fn render_source(&self, pal: Palette, cx: &mut Context<Self>) -> AnyElement {
        Button::new("insert-ocr-drop")
            .ghost()
            .w_full()
            .h(px(SOURCE_HEIGHT))
            .p_0()
            .overflow_hidden()
            .border_1()
            .border_color(pal.panel_border)
            .bg(pal.panel)
            .drag_over::<ExternalPaths>(move |style, _, _, _| {
                style.border_color(pal.foreground).bg(pal.hover)
            })
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.use_file(paths.paths(), window, cx)
            }))
            .on_click(cx.listener(|this, _, window, cx| this.pick_file(window, cx)))
            .map(|this| match &self.file {
                Some(path) => this.child(
                    img(crate::thumbnail::source(
                        path.clone(),
                        self.file_modified,
                        px(DIALOG_WIDTH),
                    ))
                    .size_full()
                    .object_fit(ObjectFit::Contain)
                    .with_fallback(move || {
                        Icon::new(icons::image())
                            .text_color(pal.muted)
                            .into_any_element()
                    }),
                ),
                None => this.child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_2()
                        .text_sm()
                        .text_color(pal.muted)
                        .child(Icon::new(icons::scan_text()).large())
                        .child(t!("insert.ocr.drop_hint")),
                ),
            })
            .into_any_element()
    }

    /// 읽기 상태와 다시 읽기 버튼.
    fn render_status(&self, pal: Palette, cx: &mut Context<Self>) -> AnyElement {
        let status: Option<(SharedString, Hsla)> = match &self.status {
            Status::Idle => None,
            Status::Recognizing => Some((t!("insert.ocr.recognizing").into(), pal.muted)),
            Status::Transcribing(model) => Some((
                t!("insert.ocr.transcribing", model = model).into(),
                pal.muted,
            )),
            Status::Recognized(0) => Some((t!("insert.ocr.no_text").into(), pal.muted)),
            Status::Recognized(count) => {
                Some((t!("insert.ocr.done", count = count).into(), pal.muted))
            }
            Status::Transcribed => Some((t!("insert.ocr.ai_done").into(), pal.muted)),
            Status::Failed(error) => Some((error.clone(), pal.danger)),
        };
        div()
            .flex()
            .items_center()
            .gap_2()
            .min_h(px(24.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_xs()
                    .when_some(status, |this, (status, color)| {
                        this.text_color(color).child(status)
                    }),
            )
            .when(self.file.is_some(), |this| {
                this.child(
                    Button::new("insert-ocr-reread")
                        .ghost()
                        .small()
                        .label(t!("insert.ocr.reread"))
                        .disabled(self.status.busy())
                        .on_click(cx.listener(|this, _, window, cx| this.read(window, cx))),
                )
            })
            .into_any_element()
    }

    /// AI로 읽기 옵션. 켜져 있으면 모델·초안·추가 지시와 전송 안내를 보인다.
    fn render_ai(&self, pal: Palette, cx: &mut Context<Self>) -> AnyElement {
        let note = |text: SharedString, color: Hsla| div().text_xs().text_color(color).child(text);
        let Some(backend) = &self.backend else {
            return div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    Checkbox::new("insert-ocr-use-ai")
                        .label(t!("insert.ocr.use_ai").to_string())
                        .checked(false)
                        .disabled(true),
                )
                .child(note(
                    if cx.global::<crate::settings::Prefs>().ai_provider
                        == Some(crate::settings::AiProvider::ClaudeCode)
                    {
                        t!("insert.ocr.ai_agent").into()
                    } else {
                        t!("insert.ocr.ai_not_configured").into()
                    },
                    pal.muted,
                ))
                .into_any_element();
        };
        let provider = match backend {
            Backend::Bedrock { .. } => "Amazon Bedrock",
            Backend::TokenPlan { .. } => "Alibaba Token Plan",
            Backend::ClaudeCode { .. } => "Claude Code",
        };
        let model: AnyElement = match &self.models {
            Models::Unavailable => note("".into(), pal.muted).into_any_element(),
            Models::Loading => {
                note(t!("insert.ocr.models_loading").into(), pal.muted).into_any_element()
            }
            Models::Failed(err) => {
                note(t!("insert.ocr.models_failed", err = err).into(), pal.danger)
                    .into_any_element()
            }
            Models::Ready(models) if models.is_empty() => {
                note(t!("insert.ocr.no_vision_models").into(), pal.danger).into_any_element()
            }
            Models::Ready(_) => Select::new(&self.model_picker)
                .small()
                .placeholder(t!("insert.ocr.model_placeholder"))
                .menu_width(px(260.0))
                .into_any_element(),
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                Checkbox::new("insert-ocr-use-ai")
                    .label(t!("insert.ocr.use_ai").to_string())
                    .checked(self.options.use_ai)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.options.use_ai = *checked;
                        this.save_options(cx);
                        cx.notify();
                    })),
            )
            .when(self.options.use_ai, |this| {
                this.child(note(
                    t!("insert.ocr.use_ai_hint", provider = provider).into(),
                    pal.muted,
                ))
                .child(form_row(pal, t!("insert.ocr.model"), model))
                .child(
                    Checkbox::new("insert-ocr-send-draft")
                        .label(t!("insert.ocr.send_draft").to_string())
                        .checked(self.options.send_draft)
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            this.options.send_draft = *checked;
                            this.save_options(cx);
                            cx.notify();
                        })),
                )
                .child(form_row(
                    pal,
                    t!("insert.ocr.instruction"),
                    Textarea::new(&self.instruction),
                ))
            })
            .into_any_element()
    }
}

impl Render for InsertOcrForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = Palette::from_theme(cx.theme());
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.render_source(pal, cx))
            .child(self.render_status(pal, cx))
            .child(
                Checkbox::new("insert-ocr-keep-breaks")
                    .label(t!("insert.ocr.keep_breaks").to_string())
                    .checked(self.options.keep_breaks)
                    .on_click(cx.listener(|this, checked: &bool, window, cx| {
                        this.options.keep_breaks = *checked;
                        this.save_options(cx);
                        // AI의 답은 다시 읽어야 바뀐다.
                        if !this.from_ai && !this.status.busy() {
                            this.refresh_preview(window, cx);
                        }
                        cx.notify();
                    })),
            )
            .child(self.render_ai(pal, cx))
            .child(
                div()
                    .text_xs()
                    .text_color(pal.muted)
                    .child(t!("insert.ocr.saved_note")),
            )
            .child(Textarea::new(&self.preview))
    }
}

/// AI에 보낼 이미지: 긴 변 [`AI_IMAGE_SIDE`]까지 줄인 PNG. 너무 크면 JPEG로 바꾼다. (바이트, JPEG인지)
///
/// `image` 크레이트로 풀고(사진의 EXIF 방향을 따른다), 풀지 못하는 형식(HEIC 등)은 QuickLook 썸네일로 연다.
fn image_for_ai(path: &Path) -> Option<(Vec<u8>, bool)> {
    let decoded = decode_oriented(path)
        .or_else(|| image::load_from_memory(&crate::thumbnail::png(path, AI_IMAGE_SIDE)?).ok())?;
    let decoded = if decoded.width().max(decoded.height()) > AI_IMAGE_SIDE {
        decoded.resize(
            AI_IMAGE_SIDE,
            AI_IMAGE_SIDE,
            image::imageops::FilterType::Lanczos3,
        )
    } else {
        decoded
    };
    let mut png = Vec::new();
    decoded
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    if png.len() <= AI_PNG_LIMIT {
        return Some((png, false));
    }
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 90)
        .encode_image(&decoded.into_rgb8())
        .ok()?;
    Some((jpeg, true))
}

/// 이미지를 풀어 EXIF 방향대로 돌린다.
fn decode_oriented(path: &Path) -> Option<image::DynamicImage> {
    use image::ImageDecoder as _;
    let mut decoder = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .into_decoder()
        .ok()?;
    let orientation = decoder.orientation().ok();
    let mut decoded = image::DynamicImage::from_decoder(decoder).ok()?;
    if let Some(orientation) = orientation {
        decoded.apply_orientation(orientation);
    }
    Some(decoded)
}

impl HanjiApp {
    /// OCR 텍스트 삽입 대화상자를 연다. 열린 시트가 없으면 안내만 한다.
    pub(super) fn on_insert_ocr_text(
        &mut self,
        _: &InsertOcrText,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = &self.editor else {
            self.notify(t!("common.open_sheet_first"), window, cx);
            return;
        };
        let target = editor.state().clone();
        let app = cx.entity().downgrade();
        let backend = Backend::llm_from_prefs(cx.global::<crate::settings::Prefs>());
        let form = cx.new(|cx| InsertOcrForm::new(backend, window, cx));
        // 미리보기 칸의 Enter는 줄바꿈이므로 확인(`on_ok`)을 걸지 않는다. 삽입은 버튼으로만 한다.
        window.open_dialog(cx, move |dialog, _, _| {
            let (form, target, app) = (form.clone(), target.clone(), app.clone());
            let closing = form.clone();
            dialog
                .w(px(DIALOG_WIDTH))
                .title(t!("insert.ocr.title"))
                .close_button(false)
                // 닫을 때 추가 지시를 저장한다(나머지 옵션은 바꿀 때 저장한다).
                .on_close(move |_, _, cx| {
                    closing.update(cx, |form, cx| form.save_options(cx));
                })
                .child(form.clone())
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("insert-ocr-cancel")
                                .label(t!("common.cancel"))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("insert-ocr-ok")
                                .primary()
                                .label(t!("common.insert"))
                                .on_click(move |_, window, cx| {
                                    form.update(cx, |form, cx| form.save_options(cx));
                                    let Some(text) = form.read(cx).text(cx) else {
                                        return;
                                    };
                                    app.update(cx, |app, cx| {
                                        app.insert_ocr_text(&target, text, window, cx)
                                    })
                                    .ok();
                                    window.close_dialog(cx);
                                }),
                        ),
                )
        });
    }

    /// 대화상자를 연 시트(`target`)가 아직 열려 있으면 커서 자리(선택이 있으면 선택 대신)에 글을 넣는다.
    fn insert_ocr_text(
        &mut self,
        target: &Entity<EditorState>,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = &self.editor else {
            return;
        };
        if editor.state() != target {
            self.notify(t!("insert.ocr.sheet_closed"), window, cx);
            return;
        }
        editor.state().update(cx, |state, cx| {
            state.replace(text, window, cx);
            state.focus(window, cx);
        });
        self.on_editor_changed(window, cx);
    }
}
