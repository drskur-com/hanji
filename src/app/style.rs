//! 화면 공통: 레이아웃 치수, 테마 토큰(Palette), 툴바 버튼 헬퍼.

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::*;
use gpui_kit::gpui::prelude::{FluentBuilder, InteractiveElement as _};
use gpui_kit::gpui::*;

// --- 레이아웃 치수 -----------------------------------------------------------
/// 컬럼 위쪽 툴바 높이. macOS 신호등 버튼도 이 줄의 세로 중앙에 맞춘다.
pub(crate) const TOOLBAR_HEIGHT: f32 = 52.0;
/// 라이브러리 패널(사이드바 1단)과 창 가장자리 사이 여백.
pub(crate) const PANEL_INSET: f32 = 8.0;
/// 라이브러리 패널 폭.
pub(crate) const PANEL_WIDTH: f32 = 220.0;
/// 패널과 시트 목록 사이 여백(그림자가 잘리지 않을 만큼).
pub(crate) const PANEL_GAP: f32 = 4.0;
/// 패널이 차지하는 전체 슬롯 폭(접기 애니메이션 대상).
pub(crate) const PANEL_SLOT_WIDTH: f32 = PANEL_INSET + PANEL_WIDTH + PANEL_GAP;
/// 사이드바 2단(시트 목록) 폭.
pub(crate) const SHEET_COLUMN_WIDTH: f32 = 280.0;
/// macOS: 패널을 접었을 때 시트 목록 툴바 왼쪽에 비워 둘 폭.
/// 신호등은 x=20에서 시작해 약 x=88에서 끝나므로, 그 뒤로 8px 여유를 둔다.
#[cfg(target_os = "macos")]
pub(crate) const TRAFFIC_LIGHT_SPACE: f32 = 96.0;

/// 사이드바 1단 접기/펴기 스프링. 임계 감쇠(ζ = 1)라 튀지 않고 약 0.3초에 멈춘다.
pub(crate) const SIDEBAR_SPRING: SpringConfig = SpringConfig::new(400.0, 40.0, 1.0);

/// 렌더에 쓰는 테마 토큰 묶음. 렌더 초입에서 값으로 복사해 `cx` 대여와 분리한다.
/// 색은 모두 테마 JSON에서 온다(ADR-0001).
#[derive(Clone, Copy)]
pub(crate) struct Palette {
    pub(crate) background: Hsla,
    pub(crate) foreground: Hsla,
    pub(crate) muted: Hsla,
    pub(crate) border: Hsla,
    pub(crate) danger: Hsla,
    /// 더한 것·성공(어시스턴트 변경 카드의 더한 줄 등).
    pub(crate) success: Hsla,
    /// 라이브러리 패널 배경/테두리. 테두리는 툴바 버튼·목록 구분선 같은 가는 선에도 쓴다.
    pub(crate) panel: Hsla,
    pub(crate) panel_border: Hsla,
    /// 패널의 선택된 줄.
    pub(crate) accent: Hsla,
    pub(crate) accent_foreground: Hsla,
    /// 목록 hover / 선택된 시트.
    pub(crate) hover: Hsla,
    pub(crate) active: Hsla,
    /// "새로운 프로젝트…" 같은 옅은 강조 버튼.
    pub(crate) tint: Hsla,
    pub(crate) tint_foreground: Hsla,
}

impl Palette {
    pub(crate) fn from_theme(theme: &Theme) -> Self {
        Self {
            background: theme.background,
            foreground: theme.foreground,
            muted: theme.muted_foreground,
            border: theme.border,
            danger: theme.danger,
            success: theme.success,
            panel: theme.sidebar,
            panel_border: theme.sidebar_border,
            accent: theme.sidebar_accent,
            accent_foreground: theme.sidebar_accent_foreground,
            hover: theme.list_hover,
            active: theme.list_active,
            tint: theme.secondary,
            tint_foreground: theme.primary,
        }
    }
}

/// 프로젝트 색(메타 이름 → 테마 색). 색은 테마에서만 가져온다(ADR-0001).
/// 첫 항목은 "색 없음"에 해당하는 본문 색이다.
pub(crate) const PROJECT_COLORS: [&str; 8] = [
    "black", "red", "yellow", "green", "cyan", "blue", "magenta", "gray",
];

/// 프로젝트 색 이름을 테마 색으로 바꾼다. 모르는 이름이면 `None`.
pub(crate) fn project_color(name: &str, theme: &Theme) -> Option<Hsla> {
    Some(match name {
        "black" => theme.foreground,
        "red" => theme.red,
        "yellow" => theme.yellow,
        "green" => theme.green,
        "cyan" => theme.cyan,
        "blue" => theme.blue,
        "magenta" => theme.magenta,
        "gray" => theme.muted_foreground,
        _ => return None,
    })
}

/// 아이콘만 있는 둥근 툴바 버튼. `enabled`가 false면 흐리게 그리고 hover/커서를 주지 않는다.
pub(crate) fn icon_button(
    id: impl Into<ElementId>,
    icon: AssetIcon,
    enabled: bool,
    pal: Palette,
) -> Stateful<Div> {
    let color = if enabled {
        pal.foreground.opacity(0.8)
    } else {
        pal.muted.opacity(0.6)
    };
    div()
        .id(id)
        .size(px(30.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        // 툴바의 창 이동(drag)으로 번지지 않게 막는다.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .when(enabled, |this| {
            this.cursor_pointer()
                .hover(move |style| style.bg(pal.hover))
        })
        .child(Icon::new(icon).small().text_color(color))
}

/// 툴바 버튼 묶음. 배경·테두리 없이 아이콘만 두고, 버튼은 마우스를 올렸을 때만 옅은 원을 그린다.
pub(crate) fn tool_group(_pal: Palette) -> Div {
    div().flex().items_center().gap(px(2.0))
}

/// macOS: 투명 타이틀바 대신 툴바를 끌어 창을 옮긴다. 더블클릭은 시스템 동작(확대/최소화).
/// 그 외 플랫폼은 커스텀 타이틀바가 따로 있으므로 그대로 둔다.
pub(crate) fn window_drag_area(element: Div) -> Div {
    #[cfg(target_os = "macos")]
    let element = element.on_mouse_down(MouseButton::Left, |event, window, _| {
        if event.click_count >= 2 {
            window.titlebar_double_click();
        } else {
            window.start_window_move();
        }
    });
    element
}
