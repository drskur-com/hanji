//! 사용자가 어시스턴트 창에 끌어다 놓은 첨부(ADR-0058): 그림·파일·폴더.
//!
//! - 그림은 보낼 때 모델이 받는 크기로 줄여 대화 기록 옆 첨부 폴더(`attachments/<대화 id>/<해시>.<확장자>`)에 쓴다.
//!   붙인 말에서만 그림으로 보내고, 그 뒤의 말에서는 그 파일의 경로를 알려 다시 보고 싶으면 읽게 한다(읽기를
//!   허락한다). 대화 내역의 썸네일도 그 파일을 그린다.
//! - 파일·폴더는 경로만 적는다. 그 대화에서는 읽기 전용으로 허락한 것으로 보고([`grants`]) 모델에 경로를 알린다.

use std::hash::{Hash as _, Hasher as _};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::Turn;
use super::web::{self, FetchedImage, ImageFormat};

/// 첨부 하나.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub kind: AttachmentKind,
    /// 끌어다 놓은 원래 경로(절대 경로).
    pub path: PathBuf,
    /// 그림이면 대화 첨부 폴더 안의 파일 이름. 쓰지 못했으면(기록이 없을 때) `None`이고 원래 경로를 읽는다.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stored: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttachmentKind {
    Image,
    File,
    Folder,
}

/// 그림으로 붙이는 확장자.
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff"];

impl Attachment {
    /// 끌어다 놓은 경로. 없는 경로면 `None`.
    pub fn from_path(path: &Path) -> Option<Self> {
        let path = path.canonicalize().ok()?;
        let meta = std::fs::metadata(&path).ok()?;
        let kind = if meta.is_dir() {
            AttachmentKind::Folder
        } else if path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| IMAGE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
        {
            AttachmentKind::Image
        } else {
            AttachmentKind::File
        };
        Some(Self {
            kind,
            path,
            stored: None,
        })
    }

    /// 보일 이름(파일·폴더 이름).
    pub fn name(&self) -> String {
        self.path.file_name().map_or_else(
            || self.path.to_string_lossy().into_owned(),
            |name| name.to_string_lossy().into_owned(),
        )
    }

    /// 그림 파일: 첨부 폴더(`dir`)에 쓴 것, 없으면 원래 파일. 둘 다 없으면 `None`.
    pub fn image_file(&self, dir: Option<&Path>) -> Option<PathBuf> {
        if self.kind != AttachmentKind::Image {
            return None;
        }
        dir.zip(self.stored.as_deref())
            .map(|(dir, name)| dir.join(name))
            .filter(|file| file.is_file())
            .or_else(|| self.path.is_file().then(|| self.path.clone()))
    }
}

/// 원래 그림을 모델이 받는 크기·형식으로 줄여 `dir`에 쓰고 파일 이름을 돌려준다. 이름은 내용의 해시라
/// 같은 그림을 다시 붙여도 한 벌만 남는다.
pub fn store_image(path: &Path, dir: &Path) -> Result<String, String> {
    let body = std::fs::read(path).map_err(|err| err.to_string())?;
    let (image, _, _) = web::prepare_image(&body)?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    image.bytes.hash(&mut hasher);
    let name = format!("{:016x}.{}", hasher.finish(), extension(image.format));
    std::fs::create_dir_all(dir).map_err(|err| err.to_string())?;
    let file = dir.join(&name);
    if !file.is_file() {
        std::fs::write(&file, &image.bytes).map_err(|err| err.to_string())?;
    }
    Ok(name)
}

/// 모델에 보낼 그림. 첨부 폴더에 쓴 것은 이미 줄였으므로 그대로 쓰고, 원래 파일이면 줄인다.
pub fn load_image(file: &Path) -> Option<FetchedImage> {
    let body = std::fs::read(file).ok()?;
    web::prepare_image(&body).ok().map(|(image, _, _)| image)
}

fn extension(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Gif => "gif",
        ImageFormat::Webp => "webp",
    }
}

/// 그림 형식의 MIME 형식.
pub fn mime(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "image/png",
        ImageFormat::Jpeg => "image/jpeg",
        ImageFormat::Gif => "image/gif",
        ImageFormat::Webp => "image/webp",
    }
}

/// 이 대화에서 읽기를 허락한 파일·폴더(처음 붙인 순서, 겹치지 않게).
pub fn grants(turns: &[Turn]) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = Vec::new();
    for attachment in turns.iter().flat_map(|turn| &turn.attachments) {
        if attachment.kind != AttachmentKind::Image && !paths.contains(&attachment.path) {
            paths.push(attachment.path.clone());
        }
    }
    paths
}

/// `path`가 허락한 파일이거나 허락한 폴더 안인지. 링크·`..`로 벗어나지 않게 실제 경로로 견준다.
pub fn granted(grants: &[PathBuf], path: &Path) -> bool {
    let Ok(path) = path.canonicalize() else {
        return false;
    };
    grants.iter().any(|grant| {
        path.starts_with(grant)
            || grant
                .canonicalize()
                .is_ok_and(|grant| path.starts_with(grant))
    })
}

/// 모델에 보낼 첨부와 사용자 말 끝에 붙일 알림 글. `fresh`면 이번에 보내는 말의 첨부라 그림을 그림으로 보낸다.
/// 앞의 말에 붙인 그림은 그 파일(`dir`의 사본, 없으면 원래 파일)을 읽기 허락할 파일로 바꾸고 경로를 알린다.
pub(super) fn for_request(
    attachments: &[Attachment],
    fresh: bool,
    dir: Option<&Path>,
) -> (Vec<Attachment>, Option<String>) {
    let mut kept = Vec::new();
    let mut lines = Vec::new();
    for attachment in attachments {
        let path = attachment.path.to_string_lossy();
        match attachment.kind {
            AttachmentKind::Image if fresh => {
                lines.push(format!(
                    "- image {} (attached as an image)",
                    attachment.name()
                ));
                kept.push(attachment.clone());
            }
            AttachmentKind::Image => match attachment.image_file(dir) {
                Some(file) => {
                    lines.push(format!(
                        "- image {} (shown when it was attached; to look at it again, read {})",
                        attachment.name(),
                        file.display()
                    ));
                    kept.push(Attachment {
                        kind: AttachmentKind::File,
                        path: file,
                        stored: None,
                    });
                }
                None => lines.push(format!(
                    "- image {} (shown when it was attached; no longer available)",
                    attachment.name()
                )),
            },
            AttachmentKind::File => {
                lines.push(format!("- file {path}"));
                kept.push(attachment.clone());
            }
            AttachmentKind::Folder => {
                lines.push(format!("- folder {path}/"));
                kept.push(attachment.clone());
            }
        }
    }
    let note = (!lines.is_empty()).then(|| {
        format!(
            "[The user attached the following. Files and folders are given as absolute paths you may read \
(read-only; do not change them):\n{}]",
            lines.join("\n")
        )
    });
    (kept, note)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_grants_and_note() {
        let dir = std::env::temp_dir().join(format!("hanji-attach-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("folder")).unwrap();
        std::fs::write(dir.join("a.md"), "a").unwrap();
        std::fs::write(dir.join("folder/b.md"), "b").unwrap();
        let png = dir.join("p.PNG");
        image::RgbImage::new(4, 4)
            .save_with_format(&png, image::ImageFormat::Png)
            .unwrap();

        let file = Attachment::from_path(&dir.join("a.md")).unwrap();
        let folder = Attachment::from_path(&dir.join("folder")).unwrap();
        let picture = Attachment::from_path(&png).unwrap();
        assert_eq!(file.kind, AttachmentKind::File);
        assert_eq!(folder.kind, AttachmentKind::Folder);
        assert_eq!(picture.kind, AttachmentKind::Image);
        assert!(Attachment::from_path(&dir.join("missing")).is_none());

        let turns = vec![Turn {
            user: true,
            attachments: vec![file.clone(), folder.clone(), picture.clone(), file.clone()],
            ..Turn::default()
        }];
        let allowed = grants(&turns);
        assert_eq!(allowed, [file.path.clone(), folder.path.clone()]);
        assert!(granted(&allowed, &dir.join("folder/b.md")));
        assert!(granted(&allowed, &dir.join("folder/../a.md")));
        assert!(!granted(&allowed, &png));

        let (kept, note) = for_request(&turns[0].attachments, true, None);
        let note = note.unwrap();
        assert!(note.contains(&format!("- folder {}/", folder.path.display())));
        assert!(note.contains("- image p.PNG (attached as an image)"));
        assert!(kept.contains(&picture));
        // 앞의 말의 그림은 읽을 파일로 바꾸고 경로를 알린다.
        let (kept, note) = for_request(std::slice::from_ref(&picture), false, None);
        assert_eq!(kept[0].kind, AttachmentKind::File);
        assert_eq!(kept[0].path, picture.path);
        assert!(
            note.unwrap()
                .contains(&format!("read {}", picture.path.display()))
        );

        // 그림은 줄여 해시 이름으로 쓰고, 같은 그림은 한 벌만 남는다.
        let store = dir.join("store");
        let name = store_image(&png, &store).unwrap();
        assert_eq!(store_image(&png, &store).unwrap(), name);
        assert!(name.ends_with(".png"));
        let stored = Attachment {
            stored: Some(name),
            ..picture
        };
        assert!(stored.image_file(Some(&store)).unwrap().starts_with(&store));
        std::fs::remove_dir_all(dir).ok();
    }
}
