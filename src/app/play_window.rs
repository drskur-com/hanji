//! 플레이 창(ADR-0038): 확장의 `play` 표를 창에서 실행한다. 키보드·마우스 입력을 받고, 확장이 상태를 들고 매
//! 프레임 `update(dt)`와 `draw(g)`를 돈다. 원고에는 아무것도 쓰지 않는다.
//!
//! 실행은 확장의 플레이 작업 스레드(`extension::PlayWorker`)가 한다. 창은 프레임마다 그 사이의 입력과 지난 시간을
//! 보내고, 마지막으로 받은 그림을 창 크기에 맞춰(비율을 지켜) 그린다. 창이 비활성이면 멈춘다. 닫으면 스레드가 끝난다.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::gpui::*;

use super::HanjiApp;
use super::editor_extension::{shapes_canvas, theme};
use super::extension_painter::RasterCache;
use super::style::Palette;
use crate::extension::{self, Extension, PlayEvent, PlayWorker, Shape};

/// 한 프레임의 시간 상한(초). 창을 오래 멈췄다 돌아와도 한 번에 크게 건너뛰지 않는다.
const MAX_DT: f32 = 0.1;

/// 캔버스 둘레 여백.
const PADDING: f32 = 12.0;

pub(super) struct PlayView {
    worker: Result<PlayWorker, String>,
    shapes: Option<Rc<Vec<Shape>>>,
    /// 실행 중 오류. 있으면 멈추고 보인다.
    error: Option<String>,
    /// 다음 프레임에 보낼 입력.
    events: Vec<PlayEvent>,
    /// 마지막으로 프레임을 보낸 때.
    last: Instant,
    /// 답을 기다리는 프레임이 있는지.
    pending: bool,
    cache: Rc<RefCell<RasterCache>>,
    /// 캔버스를 그린 자리(마우스 좌표를 캔버스 좌표로 바꾼다).
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    focus: FocusHandle,
    _activation: Subscription,
}

impl PlayView {
    fn new(
        root: &std::path::Path,
        name: &str,
        args: Vec<(String, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let worker = extension::read_source(root, name)
            .and_then(|source| PlayWorker::spawn(name, source, args));
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        // 창으로 돌아오면 다시 돈다(비활성인 동안은 프레임을 요청하지 않고 소리도 멈춘다).
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            let active = window.is_window_active();
            if let Ok(worker) = &this.worker {
                worker.set_sound_paused(!active);
            }
            if active {
                this.last = Instant::now();
                cx.notify();
            }
        });
        Self {
            worker,
            shapes: None,
            error: None,
            events: Vec::new(),
            last: Instant::now(),
            pending: false,
            cache: Rc::default(),
            bounds: Rc::default(),
            focus,
            _activation: activation,
        }
    }

    /// 창 좌표를 캔버스 좌표로.
    fn canvas_point(&self, position: Point<Pixels>) -> Option<(f32, f32)> {
        let bounds = self.bounds.get()?;
        let size = self.worker.as_ref().ok()?.size;
        let scale = f32::from(bounds.size.width) / size.0.max(1.);
        let local = position - bounds.origin;
        Some((f32::from(local.x) / scale, f32::from(local.y) / scale))
    }

    fn push_mouse(&mut self, position: Point<Pixels>, make: impl FnOnce(f32, f32) -> PlayEvent) {
        if let Some((x, y)) = self.canvas_point(position) {
            self.events.push(make(x, y));
        }
    }
}

/// LÖVE의 마우스 단추 번호(1 왼쪽, 2 오른쪽, 3 가운데).
fn button_number(button: MouseButton) -> u8 {
    match button {
        MouseButton::Left => 1,
        MouseButton::Right => 2,
        MouseButton::Middle => 3,
        _ => 4,
    }
}

impl Render for PlayView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = Palette::from_theme(cx.theme());
        if let Ok(worker) = &self.worker {
            if let Some(result) = worker.take() {
                self.pending = false;
                match result {
                    Ok(shapes) => self.shapes = Some(Rc::new(shapes)),
                    Err(err) => self.error = Some(err),
                }
            }
            // 비활성인 동안은 시간을 멈춘다. 첫 그림은 그래도 그린다.
            let active = window.is_window_active();
            if self.error.is_none() && !self.pending && (active || self.shapes.is_none()) {
                let dt = if active {
                    self.last.elapsed().as_secs_f32().min(MAX_DT)
                } else {
                    0.
                };
                self.last = Instant::now();
                worker.frame(dt, std::mem::take(&mut self.events), theme(pal));
                self.pending = true;
            }
            if self.error.is_none() && (active || self.pending) {
                window.request_animation_frame();
            }
        }
        let body: AnyElement = match (&self.worker, &self.error) {
            (Err(err), _) | (Ok(_), Some(err)) => div()
                .p_4()
                .text_sm()
                .text_color(pal.danger)
                .child(err.clone())
                .into_any_element(),
            (Ok(worker), None) => {
                let viewport = window.viewport_size();
                let (w, h) = worker.size;
                let scale = ((f32::from(viewport.width) - PADDING * 2.) / w)
                    .min((f32::from(viewport.height) - PADDING * 2.) / h)
                    .max(0.1);
                shapes_canvas(
                    self.shapes.clone(),
                    self.cache.clone(),
                    Some(self.bounds.clone()),
                    worker.size,
                    px(w * scale),
                    px(h * scale),
                )
                .into_any_element()
            }
        };
        div()
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(pal.background)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, _| {
                if !event.is_held {
                    this.events
                        .push(PlayEvent::KeyPressed(event.keystroke.key.clone()));
                }
            }))
            .on_key_up(cx.listener(|this, event: &KeyUpEvent, _, _| {
                this.events
                    .push(PlayEvent::KeyReleased(event.keystroke.key.clone()));
            }))
            .on_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, window, cx| {
                window.focus(&this.focus, cx);
                let button = button_number(event.button);
                this.push_mouse(event.position, |x, y| PlayEvent::MousePressed {
                    x,
                    y,
                    button,
                });
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, _, _| {
                    let button = button_number(event.button);
                    this.push_mouse(event.position, |x, y| PlayEvent::MouseReleased {
                        x,
                        y,
                        button,
                    });
                }),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|this, event: &MouseUpEvent, _, _| {
                    let button = button_number(event.button);
                    this.push_mouse(event.position, |x, y| PlayEvent::MouseReleased {
                        x,
                        y,
                        button,
                    });
                }),
            )
            .on_mouse_up(
                MouseButton::Middle,
                cx.listener(|this, event: &MouseUpEvent, _, _| {
                    let button = button_number(event.button);
                    this.push_mouse(event.position, |x, y| PlayEvent::MouseReleased {
                        x,
                        y,
                        button,
                    });
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, _| {
                this.push_mouse(event.position, |x, y| PlayEvent::MouseMoved { x, y });
            }))
            .child(body)
    }
}

impl HanjiApp {
    /// 확장 `name`의 `play`를 블록 인자 `args`로 새 창에서 실행한다.
    pub(super) fn play_extension(
        &mut self,
        name: &str,
        args: Vec<(String, String)>,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.library.as_ref().map(|lib| lib.root.clone()) else {
            return;
        };
        let size = Extension::open(&root, name)
            .and_then(|ext| ext.play_size(&args))
            .unwrap_or((480.0, 320.0));
        let bounds = Bounds::centered(
            None,
            gpui_kit::gpui::size(px(size.0 + PADDING * 2.), px(size.1 + PADDING * 2. + 28.)),
            cx,
        );
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some(format!("▶ {name}").into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let name = name.to_string();
        if let Err(err) = gpui_kit::open_window(options, cx, move |window, cx| {
            cx.new(|cx| PlayView::new(&root, &name, args, window, cx))
        }) {
            eprintln!("플레이 창을 열지 못했습니다: {err}");
        }
    }
}
