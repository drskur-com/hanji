//! 받아쓰기(ADR-0054): 사이드바의 받아쓰기 그룹(`STT/`)과 에디터 칸 아래의 녹음 막대.
//!
//! 녹음 상태는 메인 창이 들고 있어 다른 시트로 옮겨 가도 녹음이 이어진다. 새 기록을 만들거나, 고른 지난
//! 기록 끝에 이어서 녹음한다. 파일은 인식 작업 스레드만 쓴다: 녹음하는 기록은 에디터에서 읽기 전용이고
//! 저장하지 않으며, 작업 스레드가 파일 끝에 덧붙인 문단을 에디터 본문 끝에도 붙인다.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::speech::{SpeechError, SpeechEvent, SpeechState, SpeechWaveform};
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Sizable as _, StyledExt as _};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::*;

use super::style::{Palette, icon_button};
use super::{HanjiApp, Selection};
use crate::dictation::{
    AudioSource, Engine, EventHandler, NemotronRecognizer, RecordEvent as Event, RecordingSetup,
    Refiner, SourceInput,
};
use crate::{icons, library};

mod ai;
pub(super) use ai::DictationAi;

/// 녹음을 멈춘 뒤 마지막 문단을 기다리는 시간. 넘으면 녹음 상태만 끝낸다(파일에는 작업 스레드가 마저 쓴다).
const STOP_TIMEOUT: Duration = Duration::from_secs(15);

/// 받아쓰기 상태(앱에 하나). 처음 녹음할 때 만들고, 엔진·모델·언어가 바뀌면 다시 만든다.
pub(super) struct Dictation {
    speech: Entity<SpeechState>,
    /// 인식기를 무엇으로 만들었는지(엔진과 모델 파일 또는 로캘).
    key: String,
    /// 받아 적을 소리. 입력(`SourceInput`)이 녹음을 시작할 때 읽는다.
    source: Rc<Cell<AudioSource>>,
    /// 이번 녹음의 기록 파일·Cohere·소리 파일. 인식기가 녹음을 시작할 때 읽는다.
    setup: Rc<RefCell<RecordingSetup>>,
    /// 인식 작업 스레드를 띄웠는지. 띄우지 못했으면 `Event::Done`이 오지 않는다.
    worker_started: Rc<Cell<bool>>,
    /// 녹음 중(또는 마무리 중)인 기록. 작업 스레드가 끝나야(`Event::Done`) 비운다.
    recording: Option<Recording>,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

struct Recording {
    path: PathBuf,
    /// 이번 녹음에서 만든 기록인지. 말없이 끝나면 지운다.
    created: bool,
    /// 받아 적은 문단 수.
    paragraphs: usize,
    started: Instant,
}

impl HanjiApp {
    /// 녹음 중(또는 마무리 중)인 기록 파일.
    pub(super) fn recording_path(&self) -> Option<&Path> {
        self.dictation
            .as_ref()?
            .recording
            .as_ref()
            .map(|recording| recording.path.as_path())
    }

    /// `paths`에 녹음 중인 기록이 있으면 알리고 `true`. 옮기거나 지우기 전에 본다.
    pub(super) fn refuse_recording_sheets(
        &mut self,
        paths: &[PathBuf],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let busy = self
            .recording_path()
            .is_some_and(|recording| paths.iter().any(|path| path == recording));
        if busy {
            self.notify(t!("dictation.busy_record"), window, cx);
        }
        busy
    }

    /// 받아쓰기 그룹을 고른다(파일 → 받아쓰기).
    pub(super) fn show_dictation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = self.library.as_ref().map(|lib| lib.stt.path.clone()) else {
            self.notify(t!("common.open_library_first"), window, cx);
            return;
        };
        self.open_project = None;
        self.selected_groups.clear();
        self.group_anchor = None;
        self.selected = Some(Selection::Group(dir));
        self.select_sheet(None, false, window, cx);
        self.load_previews(cx);
    }

    /// 받아쓰기 상태를 (없거나 설정이 바뀌었으면) 만든다. Nemotron을 골랐는데 모델이 없으면 안내하고 설정 창을
    /// 연다.
    fn ensure_dictation(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        // 인식 작업 스레드가 모델을 불러오기 전에 백엔드를 등록한다.
        crate::dictation::init_backends();
        let config = crate::config::Config::load().dictation;
        let engine = if Engine::available().contains(&config.engine) {
            config.engine
        } else {
            Engine::Nemotron
        };
        let model = match engine {
            Engine::Apple => None,
            Engine::Nemotron => {
                let store = crate::dictation::ModelStore::global(cx);
                store.update(cx, |store, cx| store.refresh(cx));
                let Some(model) = store
                    .read(cx)
                    .downloaded(&crate::dictation::NEMOTRON)
                    .cloned()
                else {
                    self.notify(t!("dictation.need_model"), window, cx);
                    crate::settings::open(cx);
                    return false;
                };
                Some(model)
            }
        };
        let locale = config.language().locale;
        let key = match &model {
            Some(model) => format!("nemotron:{}", model.display()),
            None => format!("apple:{locale}"),
        };
        if self
            .dictation
            .as_ref()
            .is_some_and(|dictation| dictation.key == key)
        {
            return true;
        }
        let saved = config.source;
        let source = Rc::new(Cell::new(if AudioSource::available().contains(&saved) {
            saved
        } else {
            AudioSource::Microphone
        }));
        let setup = Rc::new(RefCell::new(RecordingSetup::default()));
        let worker_started = Rc::new(Cell::new(false));
        // 작업 스레드의 소식은 창 없이(`&mut App`) 온다. 에디터에 글을 붙이려면 창이 있어야 한다.
        let (app, handle) = (cx.entity().downgrade(), window.window_handle());
        let on_event: EventHandler = Rc::new(move |event, cx| {
            let app = app.clone();
            let _ = handle.update(cx, |_, window, cx| {
                app.update(cx, |this, cx| this.on_dictation_event(event, window, cx))
            });
        });
        let input = SourceInput {
            source: source.clone(),
        };
        let speech = match model {
            Some(model) => speech_state(
                NemotronRecognizer {
                    model,
                    setup: setup.clone(),
                    started: worker_started.clone(),
                    on_event,
                },
                input,
                cx,
            ),
            #[cfg(target_os = "macos")]
            None => speech_state(
                crate::dictation::AppleRecognizer {
                    locale: locale.to_string(),
                    setup: setup.clone(),
                    started: worker_started.clone(),
                    on_event,
                },
                input,
                cx,
            ),
            #[cfg(not(target_os = "macos"))]
            None => unreachable!("macOS 받아쓰기는 macOS에서만 고른다"),
        };
        let subscriptions = vec![
            cx.subscribe(&speech, |this, _, event: &SpeechEvent, cx| {
                if let Some(dictation) = &mut this.dictation {
                    match event {
                        SpeechEvent::Started => dictation.error = None,
                        SpeechEvent::Error(error) => dictation.error = Some(error_message(error)),
                        _ => {}
                    }
                }
                cx.notify();
            }),
            // 파형·지금 말하는 글·지난 시간을 다시 그린다.
            cx.observe(&speech, |_, _, cx| cx.notify()),
        ];
        self.dictation = Some(Dictation {
            speech,
            key,
            source,
            setup,
            worker_started,
            recording: None,
            error: None,
            _subscriptions: subscriptions,
        });
        true
    }

    /// 녹음을 시작한다. `target`이 없으면 새 기록을 만들고, 있으면 그 기록 끝에 잇는다.
    pub(super) fn start_dictation(
        &mut self,
        target: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((root, dir)) = self
            .library
            .as_ref()
            .map(|lib| (lib.root.clone(), lib.stt.path.clone()))
        else {
            self.notify(t!("common.open_library_first"), window, cx);
            return;
        };
        if self.recording_path().is_some() || !self.ensure_dictation(window, cx) {
            return;
        }
        let created = target.is_none();
        let path = match target {
            Some(path) => {
                // 열어 둔 기록이면 고친 것을 먼저 저장한다. 녹음하는 동안은 작업 스레드만 쓴다.
                if self
                    .editor
                    .as_ref()
                    .is_some_and(|editor| editor.path() == path)
                    && let Err(err) = self.save_open_sheet(cx)
                {
                    self.notify(t!("editor.save_failed", err = err), window, cx);
                    return;
                }
                match crate::dictation::ensure_record_title(&path) {
                    Ok(true) => self.reload_recorded_sheet(&path, window, cx),
                    Ok(false) => {}
                    Err(err) => {
                        self.notify(t!("dictation.create_failed", err = err), window, cx);
                        return;
                    }
                }
                path
            }
            None => match crate::dictation::create_record(&dir) {
                Ok(path) => {
                    self.apply_scan(&root, library::scan(&root), window, cx);
                    self.selected = Some(Selection::Group(dir.clone()));
                    self.select_sheet(Some(path.clone()), false, window, cx);
                    path
                }
                Err(err) => {
                    self.notify(t!("dictation.create_failed", err = err), window, cx);
                    return;
                }
            },
        };

        self.start_summary_job(&path, cx);
        let Some(dictation) = &mut self.dictation else {
            return;
        };
        // Cohere(ADR-0056): 켜고 내려받았으면 문단을 고른 언어로 다시 받아 적는다.
        let config = crate::config::Config::load().dictation;
        let cohere = crate::dictation::ModelStore::global(cx)
            .read(cx)
            .downloaded(&crate::dictation::COHERE)
            .cloned()
            .filter(|_| config.cohere);
        *dictation.setup.borrow_mut() = RecordingSetup {
            target: path.clone(),
            refiner: cohere.map(|model| Refiner {
                model,
                language: config.language().code.to_string(),
            }),
            audio_file: config
                .save_audio
                .then(|| crate::dictation::audio_path(&path))
                .flatten(),
        };
        dictation.worker_started.set(false);
        dictation.error = None;
        dictation.recording = Some(Recording {
            path: path.clone(),
            created,
            paragraphs: 0,
            started: Instant::now(),
        });
        let speech = dictation.speech.clone();
        speech.update(cx, |speech, cx| speech.start(cx));
        self.sync_recording_readonly(cx);
        // 인식기를 띄우지 못했으면 작업 스레드의 끝 소식이 오지 않으니 바로 정리한다.
        if self
            .dictation
            .as_ref()
            .is_some_and(|dictation| !dictation.worker_started.get())
        {
            self.finish_recording(window, cx);
        }
        cx.notify();
    }

    /// 녹음을 멈춘다. 마지막 문단은 작업 스레드가 마저 쓴다.
    pub(super) fn stop_dictation(&mut self, cx: &mut Context<Self>) {
        if let Some(dictation) = &self.dictation {
            dictation.speech.update(cx, |speech, cx| speech.stop(cx));
        }
    }

    fn on_dictation_event(&mut self, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            Event::Written(text) => {
                if let Some(recording) = self.recording_mut() {
                    recording.paragraphs += 1;
                }
                self.on_written(&text, window, cx);
                self.on_summary_written(&text, cx);
            }
            Event::Done => self.finish_recording(window, cx),
        }
    }

    fn recording_mut(&mut self) -> Option<&mut Recording> {
        self.dictation
            .as_mut()
            .and_then(|dictation| dictation.recording.as_mut())
    }

    /// 쓰기 스레드가 기록 끝에 덧붙인 문단을 열린 에디터와 목록 미리보기에도 반영한다. 에디터 본문은 쓰기
    /// 스레드가 든 기록과 줄이 같다(녹음 전에 저장했고, 녹음 중에는 읽기 전용이다).
    fn on_written(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.recording_path().map(Path::to_path_buf) else {
            return;
        };
        if let Some(editor) = self.editor.as_ref().filter(|editor| editor.path() == path) {
            let paragraph = text.to_string();
            editor.state().update(cx, |state, cx| {
                let value = state.value();
                let insert = if value.is_empty() || value.ends_with('\n') {
                    paragraph
                } else {
                    format!("\n{paragraph}")
                };
                let at = value.len();
                state.set_selected_range(at..at, cx);
                state.replace(insert, window, cx);
                // 붙인 글이 보이게 맨 아래로 내린다(넘치는 값은 다음 레이아웃에서 맨 아래로 맞춰진다).
                let x = state.scroll_offset().x;
                state.set_scroll_offset(point(x, px(-1.0e9)), cx);
            });
            // 프로그램으로 넣은 글은 변경 소식이 없어 단어 수·마크업을 직접 맞춘다(저장은 하지 않는다).
            self.on_editor_changed(window, cx);
        }
        if let Some(sheet) = self
            .library
            .as_mut()
            .and_then(|lib| lib.stt.sheets.iter_mut().find(|sheet| sheet.path == path))
        {
            sheet.modified = library::modified_nanos(&path);
        }
        self.load_previews(cx);
        cx.notify();
    }

    /// 작업 스레드가 끝났다: 말없이 끝난 새 기록은 지우고, 에디터를 다시 쓸 수 있게 한다.
    fn finish_recording(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(recording) = self
            .dictation
            .as_mut()
            .and_then(|dictation| dictation.recording.take())
        else {
            return;
        };
        if recording.created && recording.paragraphs == 0 {
            if self.selected_sheet.as_ref() == Some(&recording.path) {
                self.select_sheet(None, false, window, cx);
            }
            if let Err(err) = std::fs::remove_file(&recording.path) {
                eprintln!("빈 받아쓰기 기록을 지우지 못했습니다: {err}");
            }
            // 말없이 끝났으니 남긴 소리도 지운다.
            if let Some(audio) = crate::dictation::audio_path(&recording.path) {
                let _ = std::fs::remove_file(audio);
            }
        } else {
            self.reload_recorded_sheet(&recording.path, window, cx);
        }
        self.sync_recording_readonly(cx);
        if let Some(root) = self.library.as_ref().map(|lib| lib.root.clone()) {
            self.apply_scan(&root, library::scan(&root), window, cx);
        }
        // 남은 원문을 마지막으로 요약한다(ADR-0055).
        self.finish_summary_job(cx);
        cx.notify();
    }

    /// 녹음을 마친 기록이 에디터에 열려 있으면 디스크와 맞춘다. 녹음 중에 열었으면 붙인 문단이
    /// 겹쳤을 수 있어, 다르면 파일에서 다시 읽는다.
    fn reload_recorded_sheet(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.as_mut().filter(|editor| editor.path() == path) else {
            return;
        };
        let Ok(text) =
            library::read_sheet(path).map(|text| crate::text_transform::to_editor(&text))
        else {
            return;
        };
        let current = editor.state().read(cx).value();
        if current.trim_end() != text.trim_end() {
            editor.replace_text(text, window, cx);
        }
        editor.mark_saved(path);
    }

    /// 녹음하는 기록이 에디터에 열려 있으면 읽기 전용으로, 아니면 쓸 수 있게 둔다.
    pub(super) fn sync_recording_readonly(&self, cx: &mut Context<Self>) {
        let Some(editor) = &self.editor else {
            return;
        };
        let readonly = self.recording_path() == Some(editor.path());
        editor
            .state()
            .update(cx, |state, cx| state.set_readonly(readonly, cx));
    }

    /// 받아 적을 소리를 바꾸고 저장한다(다음 녹음부터).
    fn set_dictation_source(&mut self, source: AudioSource, cx: &mut Context<Self>) {
        let mut config = crate::config::Config::load();
        config.dictation.source = source;
        if let Err(err) = config.save() {
            eprintln!("설정 저장 실패: {err}");
        }
        if let Some(dictation) = &self.dictation {
            dictation.source.set(source);
        }
        cx.notify();
    }

    /// 사이드바 줄 끝의 녹음 표시(빨간 점과 지난 시간). 녹음 중이 아니면 `None`.
    pub(super) fn dictation_badge(&self, cx: &App, pal: Palette) -> Option<impl IntoElement> {
        let dictation = self.dictation.as_ref()?;
        let recording = dictation.recording.as_ref()?;
        let capturing = dictation.speech.read(cx).status().is_capturing();
        Some(
            div()
                .ml_auto()
                .flex()
                .items_center()
                .gap_1p5()
                .text_xs()
                .font_normal()
                .text_color(pal.muted)
                .child(recording_dot(pal))
                .when(capturing, |this| {
                    this.child(crate::media::format_time(recording.started.elapsed()))
                }),
        )
    }

    /// 에디터 칸 아래의 녹음 막대. 받아쓰기 그룹을 보고 있거나 녹음 중일 때만 보인다.
    pub(super) fn render_dictation_bar(
        &self,
        cx: &mut Context<Self>,
        pal: Palette,
    ) -> Option<AnyElement> {
        let lib = self.library.as_ref()?;
        let viewing = self.effective_selection() == Some(Selection::Group(lib.stt.path.clone()));
        let recording = self
            .dictation
            .as_ref()
            .and_then(|dictation| dictation.recording.as_ref());
        if !viewing && recording.is_none() {
            return None;
        }
        let error = self
            .dictation
            .as_ref()
            .and_then(|dictation| dictation.error.clone());

        // 요약 칸은 녹음 중이면 그 기록, 아니면 고른 기록의 것이다.
        let summary = recording
            .map(|recording| recording.path.clone())
            .or_else(|| self.selected_record())
            .and_then(|record| self.render_summary_panel(&record, cx, pal));
        let content = match (recording, &self.dictation) {
            (Some(recording), Some(dictation)) => {
                self.render_recording(dictation, recording, cx, pal)
            }
            _ => self.render_record_buttons(cx, pal),
        };

        Some(
            div()
                .flex_shrink_0()
                .px_4()
                .py_3()
                .flex()
                .flex_col()
                .gap_2()
                .border_t_1()
                .border_color(pal.panel_border)
                .bg(pal.background)
                .children(summary)
                .child(content)
                .when_some(error, |this, error| {
                    this.child(div().text_sm().text_color(pal.danger).child(error))
                })
                .into_any_element(),
        )
    }

    /// 녹음하지 않을 때: 소리 고르기, 녹음(고른 기록에 잇거나 새 기록).
    fn render_record_buttons(&self, cx: &mut Context<Self>, pal: Palette) -> AnyElement {
        let current = self
            .dictation
            .as_ref()
            .map(|dictation| dictation.source.get())
            .unwrap_or_else(|| crate::config::Config::load().dictation.source);
        let choices = AudioSource::available();
        let hint = match current {
            AudioSource::Microphone => None,
            AudioSource::System => Some(if cfg!(target_os = "windows") {
                t!("dictation.source.hint_system_windows")
            } else {
                t!("dictation.source.hint_system")
            }),
            AudioSource::Both => Some(t!("dictation.source.hint_both")),
        };
        // 받아쓰기 기록을 골랐으면 그 끝에 이어서 녹음하고, 아니면 새 기록을 만든다.
        let target = self.selected_record();
        let tools = target
            .as_deref()
            .and_then(|record| self.render_record_tools(record, cx, pal));
        // 고른 기록의 녹음 소리(설정에서 남기기를 켰을 때)를 시스템 앱으로 연다.
        let audio = target
            .as_deref()
            .and_then(crate::dictation::audio_path)
            .filter(|path| path.is_file());

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    // 녹음 중의 일시 정지 버튼과 같은 자리(왼쪽 끝)에 둔다.
                    .child(
                        record_button("dictation-record", pal)
                            .tooltip(|window, cx| {
                                Tooltip::new(t!("dictation.record").to_string()).build(window, cx)
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.start_dictation(target.clone(), window, cx)
                            })),
                    )
                    .when(choices.len() > 1, |this| {
                        this.child(self.render_source_picker(choices, current, cx))
                    })
                    .when_some(audio, |this, audio| {
                        this.child(div().flex_1()).child(
                            Button::new("dictation-listen")
                                .small()
                                .ghost()
                                .icon(icons::play())
                                .label(t!("dictation.listen"))
                                .on_click(move |_, _, cx| cx.open_with_system(&audio)),
                        )
                    }),
            )
            .children(hint.map(|hint| div().text_xs().text_color(pal.muted).child(hint)))
            .children(tools)
            .into_any_element()
    }

    /// 받아 적을 소리 고르기: 마이크·시스템 소리·둘 다를 아이콘과 함께 한 줄로 고른다.
    fn render_source_picker(
        &self,
        choices: &'static [AudioSource],
        current: AudioSource,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let app = cx.entity().downgrade();
        TabBar::new("dictation-source")
            .segmented()
            .small()
            .selected_index(
                choices
                    .iter()
                    .position(|&source| source == current)
                    .unwrap_or(0),
            )
            .on_click(move |index: &usize, _, cx| {
                if let Some(&source) = choices.get(*index) {
                    app.update(cx, |this, cx| this.set_dictation_source(source, cx))
                        .ok();
                }
            })
            .children(choices.iter().map(|&source| {
                // `Tab::icon`은 라벨 없는 아이콘 탭이 된다.
                let (marks, label): (&[IconName], _) = match source {
                    AudioSource::Microphone => {
                        (&[icons::microphone()], t!("dictation.source.microphone"))
                    }
                    AudioSource::System => {
                        (&[icons::system_audio()], t!("dictation.source.system"))
                    }
                    // 두 소리를 섞으므로 두 아이콘을 나란히 보인다.
                    AudioSource::Both => (
                        &[icons::microphone(), icons::system_audio()],
                        t!("dictation.source.both"),
                    ),
                };
                // 앞붙이는 고른 칸의 배경 밖에 놓이므로 아이콘과 글을 탭 안에 함께 둔다.
                Tab::new().child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_0p5()
                                .children(marks.iter().map(|&icon| Icon::new(icon).small())),
                        )
                        .child(label),
                )
            }))
            .into_any_element()
    }

    /// 녹음 중: 일시 정지, 파형, 지난 시간, 요약 보기, 지금 말하는 글.
    fn render_recording(
        &self,
        dictation: &Dictation,
        recording: &Recording,
        cx: &mut Context<Self>,
        pal: Palette,
    ) -> AnyElement {
        let speech = dictation.speech.read(cx);
        let status = speech.status();
        let hypothesis = speech.transcript();
        // 멈춘 기록은 골라서 다시 녹음하면 끝에 이어진다. 마지막 문단을 맺는 동안은 누를 수 없다.
        let capturing = status.is_capturing();
        let button = icon_button("dictation-pause", icons::pause(), capturing, pal)
            .border_1()
            .border_color(pal.panel_border)
            .tooltip(move |window, cx| {
                let label = if capturing {
                    t!("dictation.pause")
                } else {
                    t!("dictation.stopping")
                };
                Tooltip::new(label.to_string()).build(window, cx)
            })
            .when(capturing, |this| {
                this.on_click(cx.listener(|this, _, _, cx| this.stop_dictation(cx)))
            });
        let elapsed = status
            .is_capturing()
            .then(|| crate::media::format_time(recording.started.elapsed()));
        let summary_button = self.render_live_summary(cx);

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(button)
                    .child(
                        SpeechWaveform::new(&dictation.speech)
                            .flex_1()
                            .with_size(px(24.0)),
                    )
                    .child(
                        div()
                            .w(px(56.0))
                            .text_right()
                            .text_sm()
                            .text_color(pal.muted)
                            .children(elapsed),
                    ),
            )
            .when_some(summary_button, |this, button| {
                this.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_end()
                        .text_xs()
                        .text_color(pal.muted)
                        .child(button),
                )
            })
            .when(!hypothesis.is_empty(), |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(pal.muted)
                        .line_clamp(3)
                        .text_ellipsis()
                        .child(hypothesis),
                )
            })
            .into_any_element()
    }

    /// 고른 받아쓰기 기록(녹음 막대 아래 기록 도구의 대상).
    fn selected_record(&self) -> Option<PathBuf> {
        self.selected_sheet.clone().filter(|path| {
            self.library
                .as_ref()
                .is_some_and(|lib| path.parent() == Some(lib.stt.path.as_path()))
        })
    }
}

/// `recognizer`로 받아 적는 녹음 상태. 음성은 기기 밖으로 보내지 않는다: 시스템 인식기(Windows는 온라인
/// 서비스)로 넘어가지 않게 한다.
fn speech_state(
    recognizer: impl gpui_kit::component::speech::SpeechRecognizer + 'static,
    input: SourceInput,
    cx: &mut App,
) -> Entity<SpeechState> {
    cx.new(|cx| {
        SpeechState::new(cx)
            .recognizer(recognizer)
            .input(input)
            .system_fallback(false)
            .stop_timeout(STOP_TIMEOUT)
    })
}

/// 녹음 버튼: 둥근 버튼 안의 빨간 점.
fn record_button(id: impl Into<ElementId>, pal: Palette) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(30.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .border_1()
        .border_color(pal.panel_border)
        .cursor_pointer()
        .hover(move |style| style.bg(pal.hover))
        .child(div().size(px(12.0)).rounded_full().bg(pal.danger))
}

/// 녹음 중 표시.
fn recording_dot(pal: Palette) -> impl IntoElement {
    div()
        .flex_shrink_0()
        .size(px(8.0))
        .rounded_full()
        .bg(pal.danger)
}

/// 녹음 실패 안내.
fn error_message(error: &SpeechError) -> SharedString {
    match error {
        SpeechError::PermissionDenied => t!("dictation.error.permission").into(),
        SpeechError::NoInputDevice => t!("dictation.error.no_device").into(),
        other => t!("dictation.error.other", err = other.to_string()).into(),
    }
}
