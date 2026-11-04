//! 번들 없이 실행할 때(`cargo run`)도 Dock에 앱 아이콘을 보인다(macOS).
//!
//! `.app` 번들은 `Info.plist`의 아이콘을 쓰지만, 맨 실행 파일은 기본 아이콘으로 뜬다.
//! 디코딩한 이미지는 계속 메모리에 남으므로(1024px이면 약 20MB) 번들에서는 건너뛰고,
//! Dock 확대에도 충분한 512px 이미지를 쓴다.

#[cfg(target_os = "macos")]
pub fn install() {
    use objc2::AllocAnyThread as _;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    let bundled = std::env::current_exe()
        .is_ok_and(|exe| exe.components().any(|part| part.as_os_str() == "Contents"));
    let Some(mtm) = MainThreadMarker::new().filter(|_| !bundled) else {
        return;
    };
    let data = NSData::with_bytes(include_bytes!("../resources/icon/hanji-512.png"));
    let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    unsafe { app.setApplicationIconImage(Some(&image)) };
}

#[cfg(not(target_os = "macos"))]
pub fn install() {}
