//! 확장이 그리는 그림 파일(`g.newImage`, ADR-0044).
//!
//! - 경로는 라이브러리 루트 기준(`Notes/Attachments/사진.jpg`)이다. 폴더 확장은 자기 폴더 안을 먼저 찾는다.
//! - 라이브러리 밖(절대 경로, `..`, 밖을 가리키는 링크)과 그림이 아닌 파일은 읽지 않는다. 확장은 픽셀을 그릴
//!   뿐 내용을 밖으로 보낼 길이 없다(네트워크·파일 쓰기가 없다).
//! - 한 번 읽은 그림은 (경로, 바뀐 때, 크기)로 캐시한다. 화면용 픽셀은 긴 변 [`MAX_SIDE`]px까지 줄이고,
//!   PDF·SVG는 원래 파일(`encoded`)을 넣는다.
//! - 움직이는 GIF는 첫 프레임으로 그리고, `img:frame(n)`·`img:frameAt(t)`가 그 프레임을 낸다(ADR-0049). 프레임은
//!   스레드마다 둔 디코더로 앞에서부터 차례로 풀고(차례로 재생하면 프레임마다 하나만 푼다), 최근 몇 장만 남긴다.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

/// 화면용 픽셀의 긴 변 상한.
pub const MAX_SIDE: u32 = 2048;

/// 읽을 수 있는 파일 크기 상한.
pub const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;

/// 캐시에 두는 그림 수.
const CACHE_LIMIT: usize = 32;

/// 그림마다 남기는 최근 프레임 수.
const FRAME_CACHE: usize = 4;

/// 스레드마다 두는 GIF 디코더 수.
const CURSOR_LIMIT: usize = 8;

/// 읽을 수 있는 확장자.
pub const EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];

/// 그림을 찾는 곳: 라이브러리 루트와 (폴더 확장이면) 그 폴더.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageRoots {
    pub library: PathBuf,
    pub folder: Option<PathBuf>,
}

/// 읽은 그림. 같은 파일이면 같은 `id`다.
pub struct ImageData {
    pub id: u64,
    /// 원래 크기(px). 확장의 좌표는 이 크기 기준이다.
    pub width: u32,
    pub height: u32,
    /// 화면용 픽셀(곱하지 않은 RGBA). 크면 줄였으므로 크기가 `width`·`height`와 다를 수 있다.
    pub rgba: image::RgbaImage,
    /// 원래 파일(PDF·SVG에 넣는다)과 그 형식(`png`, `jpeg`, …). GIF의 프레임은 처음 쓸 때 PNG로 만든다.
    encoded: OnceLock<Arc<Vec<u8>>>,
    pub format: &'static str,
    /// 그리는 쪽이 만들어 두는 것(vello 픽셀 등).
    pub(super) prepared: OnceLock<Arc<vello_cpu::Pixmap>>,
    /// 프레임이 여럿인 GIF의 프레임 수·표시 시간·반복.
    pub animation: Option<crate::gif::Info>,
    /// 최근에 푼 프레임(번호, 그림).
    frames: Mutex<Vec<(usize, Arc<ImageData>)>>,
}

impl ImageData {
    /// PDF·SVG에 넣을 파일.
    pub fn encoded(&self) -> &Arc<Vec<u8>> {
        self.encoded.get_or_init(|| {
            let mut png = Vec::new();
            self.rgba
                .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
                .ok();
            Arc::new(png)
        })
    }

    /// 움직이는 GIF의 `index`번(0부터) 프레임. 움직이지 않는 그림은 자기 자신이다.
    pub fn frame(self: &Arc<Self>, index: usize) -> Result<Arc<ImageData>, String> {
        let Some(info) = &self.animation else {
            return Ok(self.clone());
        };
        let index = index.min(info.frames() - 1);
        let mut frames = self.frames.lock().unwrap_or_else(|err| err.into_inner());
        if let Some((_, frame)) = frames.iter().find(|(ix, _)| *ix == index) {
            return Ok(frame.clone());
        }
        let rgba = decode_frame(self, index)
            .ok_or_else(|| format!("frame {} of the GIF cannot be read", index + 1))?;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (self.id, index).hash(&mut hasher);
        let frame = Arc::new(ImageData {
            id: hasher.finish(),
            width: self.width,
            height: self.height,
            rgba: shrink(rgba),
            encoded: OnceLock::new(),
            format: "png",
            prepared: OnceLock::new(),
            animation: None,
            frames: Mutex::default(),
        });
        if frames.len() >= FRAME_CACHE {
            frames.remove(0);
        }
        frames.push((index, frame.clone()));
        Ok(frame)
    }
}

/// 이 스레드의 GIF 디코더: 다음에 낼 프레임 번호와 프레임들.
struct FrameCursor {
    next: usize,
    frames: crate::gif::Frames,
}

thread_local! {
    /// 그림 `id`별 디코더. 프레임 반복자는 스레드를 넘지 못해 스레드마다 둔다.
    static CURSORS: RefCell<HashMap<u64, FrameCursor>> = RefCell::default();
}

/// `index`번 프레임을 합성해 푼다. 앞으로 가면 이어서 풀고, 뒤로 가면 처음부터 푼다.
fn decode_frame(image: &ImageData, index: usize) -> Option<image::RgbaImage> {
    CURSORS.with(|cursors| {
        let mut cursors = cursors.borrow_mut();
        let mut cursor = cursors
            .remove(&image.id)
            .filter(|cursor| cursor.next <= index)
            .unwrap_or_else(|| FrameCursor {
                next: 0,
                frames: crate::gif::Frames::new(image.encoded().clone(), crate::gif::Repeat::Once),
            });
        while cursor.next < index {
            cursor.frames.next()?;
            cursor.next += 1;
        }
        let frame = cursor.frames.next()?;
        cursor.next += 1;
        if cursors.len() >= CURSOR_LIMIT {
            cursors.clear();
        }
        cursors.insert(image.id, cursor);
        Some(frame.into_buffer())
    })
}

/// 화면용 픽셀: 긴 변을 [`MAX_SIDE`] 안으로 줄인다.
fn shrink(rgba: image::RgbaImage) -> image::RgbaImage {
    let (width, height) = rgba.dimensions();
    if width.max(height) <= MAX_SIDE {
        return rgba;
    }
    let fit = MAX_SIDE as f32 / width.max(height) as f32;
    let w = ((width as f32 * fit).round() as u32).max(1);
    let h = ((height as f32 * fit).round() as u32).max(1);
    image::imageops::resize(&rgba, w, h, image::imageops::FilterType::Triangle)
}

impl PartialEq for ImageData {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl fmt::Debug for ImageData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Image({}×{}, #{:x})", self.width, self.height, self.id)
    }
}

/// 라이브러리 기준 경로를 실제 파일로. 라이브러리 밖이거나 그림이 아니면 까닭을 돌려준다.
pub fn resolve(roots: &ImageRoots, path: &str) -> Result<PathBuf, String> {
    let relative = Path::new(path);
    if path.trim().is_empty()
        || !relative
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
    {
        return Err(format!(
            "image paths are relative to the library (like \"Notes/Attachments/photo.jpg\"), without \"..\": \"{path}\""
        ));
    }
    let extension = relative
        .extension()
        .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if !EXTENSIONS.contains(&extension.as_str()) {
        return Err(format!(
            "\"{path}\" is not an image (use {})",
            EXTENSIONS.join(", ")
        ));
    }
    let library = roots
        .library
        .canonicalize()
        .map_err(|_| "the library folder is missing".to_string())?;
    let candidates = roots
        .folder
        .iter()
        .map(|folder| folder.join(relative))
        .chain([roots.library.join(relative)]);
    for candidate in candidates {
        let Ok(real) = candidate.canonicalize() else {
            continue;
        };
        // 링크로 라이브러리 밖을 가리키면 읽지 않는다.
        if real.starts_with(&library) && real.is_file() {
            return Ok(real);
        }
    }
    Err(format!("image not found: \"{path}\""))
}

/// 그림을 읽는다(캐시).
pub fn load(roots: &ImageRoots, path: &str) -> Result<Arc<ImageData>, String> {
    let real = resolve(roots, path)?;
    let meta = std::fs::metadata(&real).map_err(|err| err.to_string())?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!(
            "\"{path}\" is too large (limit {} MB)",
            MAX_FILE_BYTES / 1024 / 1024
        ));
    }
    let stamp = (
        meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        meta.len(),
    );
    let mut cache = cache().lock().unwrap_or_else(|err| err.into_inner());
    if let Some((cached_stamp, image)) = cache.get(&real)
        && *cached_stamp == stamp
    {
        return Ok(image.clone());
    }
    let encoded = std::fs::read(&real).map_err(|err| err.to_string())?;
    let image =
        Arc::new(decode(&real, stamp, encoded).map_err(|err| format!("\"{path}\": {err}"))?);
    if cache.len() >= CACHE_LIMIT {
        cache.clear();
    }
    cache.insert(real, (stamp, image.clone()));
    Ok(image)
}

fn cache() -> &'static Mutex<HashMap<PathBuf, ((SystemTime, u64), Arc<ImageData>)>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, ((SystemTime, u64), Arc<ImageData>)>>> =
        OnceLock::new();
    CACHE.get_or_init(Mutex::default)
}

fn decode(path: &Path, stamp: (SystemTime, u64), encoded: Vec<u8>) -> Result<ImageData, String> {
    let format = image::guess_format(&encoded).map_err(|err| err.to_string())?;
    let name = match format {
        image::ImageFormat::Png => "png",
        image::ImageFormat::Jpeg => "jpeg",
        image::ImageFormat::Gif => "gif",
        image::ImageFormat::WebP => "webp",
        other => return Err(format!("{other:?} images are not supported")),
    };
    let decoded =
        image::load_from_memory_with_format(&encoded, format).map_err(|err| err.to_string())?;
    let (width, height) = (decoded.width(), decoded.height());
    if width == 0 || height == 0 {
        return Err("the image is empty".into());
    }
    let rgba = shrink(decoded.into_rgba8());
    let animation = (format == image::ImageFormat::Gif)
        .then(|| crate::gif::scan(&encoded))
        .flatten()
        .filter(|info| info.frames() > 1);
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (path, stamp).hash(&mut hasher);
    Ok(ImageData {
        id: hasher.finish(),
        width,
        height,
        rgba,
        encoded: OnceLock::from(Arc::new(encoded)),
        format: name,
        prepared: OnceLock::new(),
        animation,
        frames: Mutex::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hanji-image-{}", ulid::Ulid::generate()));
        std::fs::create_dir_all(dir.join("Notes/Attachments")).unwrap();
        std::fs::create_dir_all(dir.join("Extensions/frame")).unwrap();
        let png = |w, h| {
            let mut bytes = Vec::new();
            image::RgbaImage::from_pixel(w, h, image::Rgba([255, 0, 0, 255]))
                .write_to(
                    &mut std::io::Cursor::new(&mut bytes),
                    image::ImageFormat::Png,
                )
                .unwrap();
            bytes
        };
        std::fs::write(dir.join("Notes/Attachments/a.png"), png(4, 2)).unwrap();
        std::fs::write(dir.join("Extensions/frame/a.png"), png(1, 1)).unwrap();
        std::fs::write(dir.join("Notes/b.txt"), "not an image").unwrap();
        dir
    }

    #[test]
    fn reads_only_images_inside_the_library() {
        let dir = library();
        let roots = ImageRoots {
            library: dir.clone(),
            folder: None,
        };
        let image = load(&roots, "Notes/Attachments/a.png").unwrap();
        assert_eq!((image.width, image.height, image.format), (4, 2, "png"));
        // 다시 읽으면 캐시의 같은 그림이다.
        assert!(Arc::ptr_eq(
            &image,
            &load(&roots, "Notes/Attachments/a.png").unwrap()
        ));
        for bad in [
            "../x.png",
            "/etc/x.png",
            "Notes/b.txt",
            "Notes/missing.png",
            "",
        ] {
            assert!(load(&roots, bad).is_err(), "{bad}");
        }
        // 폴더 확장은 자기 폴더를 먼저 찾는다.
        let folder = ImageRoots {
            library: dir.clone(),
            folder: Some(dir.join("Extensions/frame")),
        };
        assert_eq!(load(&folder, "a.png").unwrap().width, 1);
        assert_eq!(load(&folder, "Notes/Attachments/a.png").unwrap().width, 4);
        #[cfg(unix)]
        {
            let outside =
                std::env::temp_dir().join(format!("hanji-outside-{}.png", ulid::Ulid::generate()));
            std::fs::copy(dir.join("Notes/Attachments/a.png"), &outside).unwrap();
            std::os::unix::fs::symlink(&outside, dir.join("Notes/link.png")).unwrap();
            assert!(load(&roots, "Notes/link.png").is_err());
            std::fs::remove_file(outside).ok();
        }
        std::fs::remove_dir_all(dir).ok();
    }
}
