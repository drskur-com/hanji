//! YouTube 카드의 동영상을 앱의 새 창에서 재생한다(ADR-0052).
//!
//! macOS에서는 WKWebView 하나만 담은 AppKit 창을 띄우고 YouTube 공식 iframe 플레이어를 넣는다. gpui 창
//! 위에 겹치지 않으므로 에디터의 스크롤·잘림과 무관하다. 같은 동영상을 다시 열면 열린 창을 앞으로 가져오고,
//! 창을 닫으면 페이지를 비워 소리를 멈춘다. 그 밖의 OS는 기본 브라우저에서 연다.

use gpui_kit::gpui::App;

/// `id` 동영상을 새 창에서 재생한다. `title`은 창 제목(비면 `YouTube`).
pub fn open(id: &str, title: &str, cx: &mut App) {
    #[cfg(target_os = "macos")]
    if macos::open(id, title) {
        return;
    }
    let _ = title;
    cx.open_url(&crate::media::youtube_watch_url(id));
}

#[cfg(target_os = "macos")]
mod macos {
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
    use objc2::{MainThreadMarker, MainThreadOnly as _};
    use objc2_app_kit::{
        NSAppearance, NSAppearanceCustomization as _, NSAppearanceNameDarkAqua, NSBackingStoreType,
        NSColor, NSWindow, NSWindowStyleMask, NSWindowTitleVisibility,
        NSWindowWillCloseNotification,
    };
    use objc2_foundation::{NSNotificationCenter, NSPoint, NSRect, NSSize, NSString, NSURL};
    use objc2_web_kit::{WKAudiovisualMediaTypes, WKWebView, WKWebViewConfiguration};
    use std::cell::RefCell;
    use std::collections::HashMap;

    /// 처음 여는 창의 본문 크기(16:9).
    const WIDTH: f64 = 960.0;
    const HEIGHT: f64 = 540.0;

    /// YouTube는 임베드 플레이어를 부른 곳(Referer)을 요구한다. 앱은 번들 ID를 주소 꼴로 보낸다.
    const ORIGIN: &str = "https://com.drskur.hanji";

    /// 열린 플레이어 창.
    struct Player {
        window: Retained<NSWindow>,
        web: Retained<WKWebView>,
        /// 창 닫힘 알림 구독. 창을 버릴 때 함께 해지한다.
        observer: Retained<ProtocolObject<dyn NSObjectProtocol>>,
    }

    thread_local! {
        /// 동영상 ID별 창. 닫힌 창은 다음에 열 때 버린다(닫히는 중에 놓으면 AppKit이 아직 쓰고 있다).
        static PLAYERS: RefCell<HashMap<String, Player>> = RefCell::new(HashMap::new());
    }

    /// 창을 띄웠으면 `true`. 메인 스레드가 아니면 `false`(브라우저로 연다).
    pub(super) fn open(id: &str, title: &str) -> bool {
        let Some(mtm) = MainThreadMarker::new() else {
            return false;
        };
        PLAYERS.with_borrow_mut(|players| {
            let center = NSNotificationCenter::defaultCenter();
            players.retain(|key, player| {
                let keep = key == id || player.window.isVisible() || player.window.isMiniaturized();
                if !keep {
                    unsafe { center.removeObserver(player.observer.as_ref()) };
                }
                keep
            });
            if let Some(player) = players.get(id) {
                if !player.window.isVisible() {
                    load(&player.web, id);
                }
                player.window.makeKeyAndOrderFront(None);
                return;
            }
            let player = create(id, title, mtm);
            player.window.makeKeyAndOrderFront(None);
            players.insert(id.to_string(), player);
        });
        true
    }

    fn create(id: &str, title: &str, mtm: MainThreadMarker) -> Player {
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, HEIGHT));
        let web = unsafe {
            let config = WKWebViewConfiguration::new(mtm);
            config.setMediaTypesRequiringUserActionForPlayback(WKAudiovisualMediaTypes::None);
            config.preferences().setElementFullscreenEnabled(true);
            WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), frame, &config)
        };
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                frame,
                NSWindowStyleMask::Titled
                    | NSWindowStyleMask::Closable
                    | NSWindowStyleMask::Miniaturizable
                    | NSWindowStyleMask::Resizable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        // 창은 `PLAYERS`가 놓는다. AppKit이 닫을 때 따로 놓으면 두 번 놓게 된다.
        unsafe { window.setReleasedWhenClosed(false) };
        let title = title.trim();
        window.setTitle(&NSString::from_str(if title.is_empty() {
            "YouTube"
        } else {
            title
        }));
        // 제목 줄은 영상처럼 검게 두고 제목 글은 숨긴다(QuickTime처럼). 제목은 윈도 메뉴·Mission Control에 보인다.
        window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        window.setTitlebarAppearsTransparent(true);
        window.setBackgroundColor(Some(&NSColor::blackColor()));
        window.setAppearance(
            NSAppearance::appearanceNamed(unsafe { NSAppearanceNameDarkAqua }).as_deref(),
        );
        window.setContentView(Some(&web));
        window.setContentAspectRatio(NSSize::new(16.0, 9.0));
        window.setContentMinSize(NSSize::new(320.0, 180.0));
        window.center();
        load(&web, id);

        let closing = web.clone();
        let block = block2::RcBlock::new(move |_| unsafe {
            closing.loadHTMLString_baseURL(&NSString::from_str(""), None);
        });
        let observer = unsafe {
            NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
                Some(NSWindowWillCloseNotification),
                Some(window.as_ref() as &AnyObject),
                None,
                &block,
            )
        };
        Player {
            window,
            web,
            observer,
        }
    }

    /// 창을 채우는 공식 플레이어 페이지를 띄운다(바로 재생).
    fn load(web: &WKWebView, id: &str) {
        let embed = crate::media::youtube_embed_url(id);
        let html = format!(
            r#"<!doctype html><html><head><meta charset="utf-8"><style>
html,body{{margin:0;height:100%;background:#000;overflow:hidden}}
iframe{{display:block;border:0;width:100%;height:100%}}
</style></head><body><iframe src="{embed}?autoplay=1&rel=0" allow="autoplay; encrypted-media; picture-in-picture; fullscreen" allowfullscreen referrerpolicy="strict-origin-when-cross-origin"></iframe></body></html>"#
        );
        let base = NSURL::URLWithString(&NSString::from_str(ORIGIN));
        unsafe { web.loadHTMLString_baseURL(&NSString::from_str(&html), base.as_deref()) };
    }
}
