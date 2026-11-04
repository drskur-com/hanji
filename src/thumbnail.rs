//! 작은 자리에 그리는 이미지(·동영상) 썸네일.
//!
//! `img(path)`는 원본을 통째로 디코딩해 메모리에 둔다. 큰 사진이 많으면 수십~수백 MB가 된다.
//! 그래서 목록·미리보기 칸에는 그 칸 크기(화면 배율 포함)로 줄인 썸네일을 쓴다.
//!
//! - macOS: QuickLook Thumbnailing(Finder와 같은 생성기). 축소 디코딩·시스템 캐시를 쓰고
//!   동영상 프레임도 만든다.
//! - 그 밖: `image` 크레이트로 디코딩한 뒤 줄인다(이미지만).
//!
//! 결과는 gpui 에셋 캐시에 (경로, 수정 시각, 크기)별로 남는다. 파일이 바뀌면 키가 달라져 새로 만든다.

use gpui_kit::gpui::*;
use std::path::PathBuf;
use std::sync::Arc;

/// 썸네일 요청 키. `size`는 화면 배율을 곱한 실제 픽셀 수(긴 변 최대).
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct ThumbnailKey {
    path: PathBuf,
    modified: u128,
    size: u32,
}

/// 썸네일 에셋.
pub enum Thumbnail {}

impl Asset for Thumbnail {
    type Source = ThumbnailKey;
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    fn load(
        key: Self::Source,
        cx: &mut App,
    ) -> impl std::future::Future<Output = Self::Output> + Send + 'static {
        let executor = cx.background_executor().clone();
        async move {
            let frame = generate(key, executor).await?;
            Ok(Arc::new(RenderImage::new([frame])))
        }
    }
}

/// `path`의 썸네일을 그리는 이미지 소스. `size`는 그릴 칸의 긴 변(논리 px)이다.
///
/// 만드는 동안이나 못 만들면 `img(...).with_fallback(...)`의 대체 요소가 보인다.
pub fn source(path: impl Into<PathBuf>, modified: u128, size: Pixels) -> ImageSource {
    let path = path.into();
    ImageSource::Custom(Arc::new(move |window, cx| {
        let pixels = (f32::from(size) * window.scale_factor()).ceil().max(1.0) as u32;
        let key = ThumbnailKey {
            path: path.clone(),
            modified,
            size: pixels,
        };
        window.use_asset::<Thumbnail>(&key, cx)
    }))
}

/// `path`의 썸네일을 PNG로 만든다(긴 변 `side` px까지). 기다리는 동안 부른 스레드를 막으므로 백그라운드
/// 작업(내보내기)에서 부른다. 동영상은 macOS에서만 된다(첫 장면, ADR-0050).
pub fn png(path: &std::path::Path, side: u32) -> Option<Vec<u8>> {
    let key = ThumbnailKey {
        path: path.to_path_buf(),
        modified: 0,
        size: side.max(1),
    };
    #[cfg(target_os = "macos")]
    let frame = futures::executor::block_on(macos::request(&key).ok()?)
        .ok()?
        .ok()?;
    #[cfg(not(target_os = "macos"))]
    let frame = Frame::new(
        image::open(&key.path)
            .ok()?
            .thumbnail(key.size, key.size)
            .into_rgba8(),
    );
    let rgba = frame.into_buffer();
    // macOS 썸네일은 BGRA다.
    #[cfg(target_os = "macos")]
    let rgba = {
        let mut rgba = rgba;
        for pixel in rgba.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
        rgba
    };
    let mut out = Vec::new();
    rgba.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .ok()?;
    Some(out)
}

fn error(message: impl Into<SharedString>) -> ImageCacheError {
    ImageCacheError::Asset(message.into())
}

/// BGRA 프레임 하나(gpui `RenderImage` 형식).
type Frame = image::Frame;

#[cfg(target_os = "macos")]
async fn generate(
    key: ThumbnailKey,
    _executor: BackgroundExecutor,
) -> Result<Frame, ImageCacheError> {
    let receiver = macos::request(&key)?;
    let image = receiver
        .await
        .map_err(|_| error("썸네일 생성이 취소되었습니다"))??;
    Ok(image)
}

#[cfg(not(target_os = "macos"))]
async fn generate(
    key: ThumbnailKey,
    executor: BackgroundExecutor,
) -> Result<Frame, ImageCacheError> {
    executor
        .spawn(async move {
            let decoded = image::open(&key.path).map_err(|err| error(err.to_string()))?;
            let mut rgba = decoded.thumbnail(key.size, key.size).into_rgba8();
            // gpui는 BGRA를 받는다.
            for pixel in rgba.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
            Ok(Frame::new(rgba))
        })
        .await
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{Frame, ThumbnailKey, error};
    use block2::RcBlock;
    use futures::channel::oneshot;
    use gpui_kit::gpui::ImageCacheError;
    use objc2::AllocAnyThread as _;
    use objc2_core_foundation::CGSize;
    use objc2_core_graphics::{
        CGBitmapContextCreate, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo,
        CGImageByteOrderInfo,
    };
    use objc2_foundation::{NSError, NSString, NSURL};
    use objc2_quick_look_thumbnailing::{
        QLThumbnailGenerationRequest, QLThumbnailGenerationRequestRepresentationTypes,
        QLThumbnailGenerator, QLThumbnailRepresentation,
    };
    use std::sync::Mutex;

    type Reply = Result<Frame, ImageCacheError>;

    /// QuickLook에 썸네일을 요청한다. 결과는 QuickLook의 백그라운드 큐에서 채널로 온다.
    pub(super) fn request(key: &ThumbnailKey) -> Result<oneshot::Receiver<Reply>, ImageCacheError> {
        let (sender, receiver) = oneshot::channel::<Reply>();
        let sender = Mutex::new(Some(sender));
        let path = key.path.to_string_lossy();
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path));
        let side = f64::from(key.size);
        // 크기에 화면 배율을 이미 곱했으므로 scale은 1로 둔다. 파일 형식 아이콘은 받지 않는다.
        let request = unsafe {
            QLThumbnailGenerationRequest::initWithFileAtURL_size_scale_representationTypes(
                QLThumbnailGenerationRequest::alloc(),
                &url,
                CGSize::new(side, side),
                1.0,
                QLThumbnailGenerationRequestRepresentationTypes::Thumbnail,
            )
        };
        let completion = RcBlock::new(
            move |representation: *mut QLThumbnailRepresentation, err: *mut NSError| {
                let reply = match unsafe { representation.as_ref() } {
                    Some(representation) => {
                        let image = unsafe { representation.CGImage() };
                        to_frame(&image).ok_or_else(|| error("썸네일을 읽을 수 없습니다"))
                    }
                    None => Err(error(match unsafe { err.as_ref() } {
                        Some(err) => err.localizedDescription().to_string(),
                        None => "썸네일이 없습니다".to_string(),
                    })),
                };
                if let Some(sender) = sender.lock().ok().and_then(|mut sender| sender.take()) {
                    let _ = sender.send(reply);
                }
            },
        );
        unsafe {
            QLThumbnailGenerator::sharedGenerator()
                .generateBestRepresentationForRequest_completionHandler(&request, &completion);
        }
        Ok(receiver)
    }

    /// CGImage를 BGRA(premultiplied 해제) 프레임으로 옮긴다.
    fn to_frame(image: &CGImage) -> Option<Frame> {
        let width = CGImage::width(Some(image));
        let height = CGImage::height(Some(image));
        if width == 0 || height == 0 {
            return None;
        }
        let bytes_per_row = width * 4;
        let mut data = vec![0u8; bytes_per_row * height];
        let space = CGColorSpace::new_device_rgb()?;
        // 메모리 순서 B,G,R,A(32비트 리틀 엔디언 + 알파 앞). CoreGraphics는 premultiplied만 그린다.
        let info = CGImageAlphaInfo::PremultipliedFirst.0 | CGImageByteOrderInfo::Order32Little.0;
        let context = unsafe {
            CGBitmapContextCreate(
                data.as_mut_ptr().cast(),
                width,
                height,
                8,
                bytes_per_row,
                Some(&space),
                info,
            )
        }?;
        let rect = objc2_core_foundation::CGRect::new(
            objc2_core_foundation::CGPoint::new(0.0, 0.0),
            CGSize::new(width as f64, height as f64),
        );
        CGContext::draw_image(Some(&context), rect, Some(image));
        drop(context);
        // gpui는 premultiplied가 아닌 알파를 받는다.
        for pixel in data.as_chunks_mut::<4>().0 {
            let alpha = u16::from(pixel[3]);
            if alpha != 0 && alpha != 255 {
                for channel in &mut pixel[..3] {
                    *channel = ((u16::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
                }
            }
        }
        let buffer = image::RgbaImage::from_raw(width as u32, height as u32, data)?;
        Some(Frame::new(buffer))
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    // `gpui::*`에 `test` 매크로가 있어 glob으로 가져오지 않는다.
    use super::{ThumbnailKey, macos};

    #[test]
    fn quicklook_makes_small_bgra_thumbnail() {
        let dir = std::env::temp_dir().join(format!("hanji-thumb-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("red.png");
        // 400×200 빨간 그림. 긴 변이 64가 되게 줄어야 한다.
        image::RgbaImage::from_pixel(400, 200, image::Rgba([255, 0, 0, 255]))
            .save(&path)
            .unwrap();

        let key = ThumbnailKey {
            path: path.clone(),
            modified: 0,
            size: 64,
        };
        let frame = futures::executor::block_on(macos::request(&key).unwrap())
            .unwrap()
            .unwrap();
        let buffer = frame.buffer();
        assert!(
            buffer.width() <= 64 && buffer.height() <= 64,
            "{:?}",
            buffer.dimensions()
        );
        assert!(buffer.width() > buffer.height());
        // BGRA: 빨강은 [0, 0, 255, 255].
        let pixel = buffer.get_pixel(buffer.width() / 2, buffer.height() / 2).0;
        assert!(
            pixel[2] > 200 && pixel[0] < 50 && pixel[3] == 255,
            "{pixel:?}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
