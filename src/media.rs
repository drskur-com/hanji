//! 시트에 넣는 동영상·오디오(ADR-0050).
//!
//! 이미지와 같은 줄 문법(`![캡션](경로){속성}`, ADR-0007)을 쓰고, 경로의 확장자로 동영상·오디오를 가른다
//! (Pandoc HTML 출력과 같은 규칙). 여기에는 gpui를 쓰지 않는 판별·헤더 읽기·포스터 만들기만 둔다.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Duration;

/// 이미지가 아닌 미디어의 종류.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MediaKind {
    Video,
    Audio,
}

/// 넣을 수 있는 동영상 확장자. 에디터는 AVFoundation(macOS)이 재생하는 형식만 받는다.
pub const VIDEO_EXTENSIONS: [&str; 3] = ["mp4", "m4v", "mov"];

/// 넣을 수 있는 오디오 확장자. 에디터는 rodio(symphonia)가 푸는 형식만 받는다.
pub const AUDIO_EXTENSIONS: [&str; 7] = ["mp3", "m4a", "aac", "wav", "flac", "ogg", "oga"];

/// 포스터(동영상 첫 장면)의 긴 변(px). 내보내기에 넣는 크기다.
pub const POSTER_SIDE: u32 = 1280;

/// 경로(또는 주소)의 확장자로 미디어 종류를 정한다. 이미지·기타는 `None`.
pub fn kind_of(destination: &str) -> Option<MediaKind> {
    let path = destination.split(['?', '#']).next().unwrap_or(destination);
    let extension = Path::new(path)
        .extension()?
        .to_string_lossy()
        .to_ascii_lowercase();
    if VIDEO_EXTENSIONS.contains(&extension.as_str()) {
        Some(MediaKind::Video)
    } else if AUDIO_EXTENSIONS.contains(&extension.as_str()) {
        Some(MediaKind::Audio)
    } else {
        None
    }
}

/// 미디어 파일의 MIME 타입(ePub manifest, PDF 첨부).
pub fn mime_type(path: &Path) -> &'static str {
    let extension = path
        .extension()
        .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "mp4" => "video/mp4",
        "m4v" => "video/x-m4v",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "ogg" | "oga" => "audio/ogg",
        _ => "application/octet-stream",
    }
}

/// 동영상 헤더에서 읽은 정보.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VideoInfo {
    /// 화면에 보이는 크기(px). 90°·270° 돌린 동영상은 가로세로를 바꿔 둔다.
    pub width: u32,
    pub height: u32,
    pub duration: Option<Duration>,
}

/// MP4·MOV(ISO BMFF)의 `moov` 상자에서 동영상 트랙 크기와 길이를 읽는다. 그림 데이터는 읽지 않는다.
pub fn video_info(path: &Path) -> Option<VideoInfo> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let moov = find_box(&mut file, 0, len, b"moov")?;
    let mut info = VideoInfo::default();
    if let Some(mvhd) = find_box(&mut file, moov.0, moov.1, b"mvhd") {
        info.duration = read_payload(&mut file, mvhd, 32).and_then(|data| movie_duration(&data));
    }
    let mut cursor = moov.0;
    while let Some((kind, start, end)) = next_box(&mut file, cursor, moov.1) {
        cursor = end;
        if &kind != b"trak" {
            continue;
        }
        let is_video = find_box(&mut file, start, end, b"mdia")
            .and_then(|mdia| find_box(&mut file, mdia.0, mdia.1, b"hdlr"))
            .and_then(|hdlr| read_payload(&mut file, hdlr, 12))
            .is_some_and(|data| data.get(8..12) == Some(b"vide"));
        if !is_video {
            continue;
        }
        let Some(tkhd) = find_box(&mut file, start, end, b"tkhd") else {
            continue;
        };
        let Some((width, height)) =
            read_payload(&mut file, tkhd, 96).and_then(|data| track_size(&data))
        else {
            continue;
        };
        info.width = width;
        info.height = height;
        return Some(info);
    }
    None
}

/// `mvhd` 내용 → 길이. 버전 0은 32비트, 1은 64비트 시간이다.
fn movie_duration(data: &[u8]) -> Option<Duration> {
    let (timescale, duration) = match data.first()? {
        0 => (
            u32::from_be_bytes(data.get(12..16)?.try_into().ok()?),
            u64::from(u32::from_be_bytes(data.get(16..20)?.try_into().ok()?)),
        ),
        1 => (
            u32::from_be_bytes(data.get(20..24)?.try_into().ok()?),
            u64::from_be_bytes(data.get(24..32)?.try_into().ok()?),
        ),
        _ => return None,
    };
    (timescale > 0 && duration > 0 && duration != u64::from(u32::MAX))
        .then(|| Duration::from_secs_f64(duration as f64 / f64::from(timescale)))
}

/// `tkhd` 내용 → 화면 크기. 끝 8바이트가 16.16 고정소수 폭·높이이고, 그 앞 36바이트가 변환 행렬이다.
fn track_size(data: &[u8]) -> Option<(u32, u32)> {
    let size = match data.first()? {
        0 => 84,
        1 => 96,
        _ => return None,
    };
    let data = data.get(..size)?;
    let fixed = |at: usize| -> Option<i32> {
        Some(i32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
    };
    let width = (fixed(size - 8)? >> 16).unsigned_abs();
    let height = (fixed(size - 4)? >> 16).unsigned_abs();
    if width == 0 || height == 0 {
        return None;
    }
    // 행렬 [a b u; c d v; x y w]의 a가 0이고 b가 ±1이면 90°·270° 돌린 영상이다.
    let matrix = size - 44;
    let (a, b) = (fixed(matrix)?, fixed(matrix + 4)?);
    if a == 0 && b.unsigned_abs() == 1 << 16 {
        Some((height, width))
    } else {
        Some((width, height))
    }
}

/// `[start, end)` 안의 상자 중 `kind`인 첫 상자의 내용 범위.
fn find_box(file: &mut File, start: u64, end: u64, kind: &[u8; 4]) -> Option<(u64, u64)> {
    let mut cursor = start;
    while let Some((found, payload, next)) = next_box(file, cursor, end) {
        if &found == kind {
            return Some((payload, next));
        }
        cursor = next;
    }
    None
}

/// `at`에서 시작하는 상자: (종류, 내용 시작, 상자 끝).
fn next_box(file: &mut File, at: u64, end: u64) -> Option<([u8; 4], u64, u64)> {
    if at + 8 > end {
        return None;
    }
    file.seek(SeekFrom::Start(at)).ok()?;
    let mut header = [0u8; 8];
    file.read_exact(&mut header).ok()?;
    let size = u64::from(u32::from_be_bytes(header[..4].try_into().ok()?));
    let kind: [u8; 4] = header[4..].try_into().ok()?;
    let (payload, box_end) = match size {
        0 => (at + 8, end),
        1 => {
            let mut large = [0u8; 8];
            file.read_exact(&mut large).ok()?;
            (at + 16, at.checked_add(u64::from_be_bytes(large))?)
        }
        size if size >= 8 => (at + 8, at + size),
        _ => return None,
    };
    (box_end <= end && payload <= box_end).then_some((kind, payload, box_end))
}

/// 상자 내용 앞 `limit`바이트까지.
fn read_payload(file: &mut File, (start, end): (u64, u64), limit: u64) -> Option<Vec<u8>> {
    let len = (end - start).min(limit);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut data = vec![0u8; len as usize];
    file.read_exact(&mut data).ok()?;
    Some(data)
}

/// 오디오 파일의 길이. 헤더만 읽어 알 수 없으면 `None`.
pub fn audio_duration(path: &Path) -> Option<Duration> {
    use rodio::Source as _;
    let decoder = rodio::Decoder::try_from(File::open(path).ok()?).ok()?;
    decoder.total_duration()
}

/// 동영상 포스터(첫 장면) PNG. 긴 변은 [`POSTER_SIDE`]까지다. 만들 수 없으면 `None`.
pub fn poster_png(path: &Path) -> Option<Vec<u8>> {
    crate::thumbnail::png(path, POSTER_SIDE)
}

/// YouTube 주소의 동영상 ID(11자). `youtu.be/ID`, `youtube.com/watch?v=ID`, `/embed/ID`, `/shorts/ID`,
/// `/live/ID`와 `m.`·`music.`·`youtube-nocookie.com`을 받는다. YouTube가 아니면 `None`(ADR-0051).
pub fn youtube_id(destination: &str) -> Option<String> {
    let url = url::Url::parse(destination.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?.to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    let mut segments = url.path_segments()?.filter(|segment| !segment.is_empty());
    let id = match host {
        "youtu.be" => segments.next()?.to_string(),
        "youtube.com" | "m.youtube.com" | "music.youtube.com" | "youtube-nocookie.com" => {
            match segments.next()? {
                "watch" => url
                    .query_pairs()
                    .find(|(key, _)| key == "v")
                    .map(|(_, value)| value.into_owned())?,
                "embed" | "shorts" | "live" | "v" => segments.next()?.to_string(),
                _ => return None,
            }
        }
        _ => return None,
    };
    (id.len() == 11
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')))
    .then_some(id)
}

/// YouTube에서 보는 주소(내보낸 책의 링크).
pub fn youtube_watch_url(id: &str) -> String {
    format!("https://www.youtube.com/watch?v={id}")
}

/// YouTube 공식 플레이어 주소(DOCX의 온라인 비디오).
pub fn youtube_embed_url(id: &str) -> String {
    format!("https://www.youtube.com/embed/{id}")
}

/// 에디터·미리보기에 보일 썸네일(480×360, 16:9 영상은 위아래에 검은 띠가 있어 16:9 칸에 채워 자른다).
pub fn youtube_thumbnail_url(id: &str) -> String {
    format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg")
}

/// 내보내기에 넣을 YouTube 썸네일(JPEG, 16:9). 큰 것(1280×720)이 없으면 작은 것(320×180)을 쓴다.
/// 내려받지 못하면(오프라인 등) `None`. 응답을 기다리는 동안 스레드를 막으므로 내보내기 작업에서 부른다.
pub fn youtube_poster(id: &str) -> Option<Vec<u8>> {
    if cfg!(test) {
        return None;
    }
    ["maxresdefault", "mqdefault"]
        .iter()
        .find_map(|name| fetch(&format!("https://i.ytimg.com/vi/{id}/{name}.jpg")))
        .filter(|bytes| crate::library::image_extension(bytes) == Some("jpg"))
}

/// HTTP GET 본문. 실패하거나 2xx가 아니면 `None`.
fn fetch(url: &str) -> Option<Vec<u8>> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    runtime.block_on(async {
        let response = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .ok()?
            .get(url)
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        response.bytes().await.ok().map(|bytes| bytes.to_vec())
    })
}

/// 재생 시간 표시(`1:05`, `1:02:03`).
pub fn format_time(time: Duration) -> String {
    let total = time.as_secs();
    let (hours, minutes, seconds) = (total / 3600, total / 60 % 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_follows_the_extension() {
        assert_eq!(kind_of("Extras/Attachments/a.MP4"), Some(MediaKind::Video));
        assert_eq!(kind_of("clip.mov"), Some(MediaKind::Video));
        assert_eq!(kind_of("song.mp3"), Some(MediaKind::Audio));
        assert_eq!(kind_of("https://x.io/a.m4a?raw=1"), Some(MediaKind::Audio));
        assert_eq!(kind_of("a.png"), None);
        assert_eq!(kind_of("noext"), None);
    }

    #[test]
    fn youtube_addresses_give_the_video_id() {
        let id = Some("dQw4w9WgXcQ".to_string());
        for url in [
            "https://youtu.be/dQw4w9WgXcQ",
            "https://youtu.be/dQw4w9WgXcQ?t=42",
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ&list=x",
            "http://youtube.com/watch?feature=share&v=dQw4w9WgXcQ",
            "https://m.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://www.youtube.com/embed/dQw4w9WgXcQ",
            "https://www.youtube.com/shorts/dQw4w9WgXcQ",
            "https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ",
        ] {
            assert_eq!(youtube_id(url), id, "{url}");
        }
        for url in [
            "https://www.youtube.com/channel/UC123",
            "https://www.youtube.com/watch?v=short",
            "https://vimeo.com/123456",
            "Attachments/clip.mp4",
            "https://evil.com/youtube.com/watch?v=dQw4w9WgXcQ",
        ] {
            assert_eq!(youtube_id(url), None, "{url}");
        }
    }

    #[test]
    fn time_is_minutes_and_seconds() {
        assert_eq!(format_time(Duration::from_secs(65)), "1:05");
        assert_eq!(format_time(Duration::from_secs(3723)), "1:02:03");
        assert_eq!(format_time(Duration::ZERO), "0:00");
    }

    /// 상자 하나(크기·종류·내용).
    fn mp4_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(payload);
        out
    }

    fn tkhd(width: u32, height: u32, rotated: bool) -> Vec<u8> {
        let mut data = vec![0u8; 84];
        let matrix: [i32; 9] = if rotated {
            [0, 1 << 16, 0, -(1 << 16), 0, 0, 0, 0, 1 << 30]
        } else {
            [1 << 16, 0, 0, 0, 1 << 16, 0, 0, 0, 1 << 30]
        };
        for (ix, value) in matrix.iter().enumerate() {
            data[40 + ix * 4..44 + ix * 4].copy_from_slice(&value.to_be_bytes());
        }
        data[76..80].copy_from_slice(&(width << 16).to_be_bytes());
        data[80..84].copy_from_slice(&(height << 16).to_be_bytes());
        mp4_box(b"tkhd", &data)
    }

    fn trak(handler: &[u8; 4], tkhd: Vec<u8>) -> Vec<u8> {
        let mut hdlr = vec![0u8; 24];
        hdlr[8..12].copy_from_slice(handler);
        let mdia = mp4_box(b"mdia", &mp4_box(b"hdlr", &hdlr));
        mp4_box(b"trak", &[tkhd, mdia].concat())
    }

    #[test]
    fn video_info_reads_the_video_track_and_movie_length() {
        let dir = std::env::temp_dir().join(format!("hanji-media-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut mvhd = vec![0u8; 100];
        mvhd[12..16].copy_from_slice(&1000u32.to_be_bytes());
        mvhd[16..20].copy_from_slice(&90_500u32.to_be_bytes());
        let moov = |video: Vec<u8>| {
            mp4_box(
                b"moov",
                &[
                    mp4_box(b"mvhd", &mvhd),
                    trak(b"soun", tkhd(0, 0, false)),
                    video,
                ]
                .concat(),
            )
        };
        let file = |name: &str, moov: Vec<u8>| {
            let path = dir.join(name);
            let bytes = [mp4_box(b"ftyp", b"isom"), mp4_box(b"mdat", &[0; 64]), moov].concat();
            std::fs::write(&path, bytes).unwrap();
            path
        };

        let plain = file("a.mp4", moov(trak(b"vide", tkhd(1920, 1080, false))));
        assert_eq!(
            video_info(&plain),
            Some(VideoInfo {
                width: 1920,
                height: 1080,
                duration: Some(Duration::from_millis(90_500)),
            })
        );
        let rotated = file("b.mov", moov(trak(b"vide", tkhd(1920, 1080, true))));
        let info = video_info(&rotated).unwrap();
        assert_eq!((info.width, info.height), (1080, 1920));
        let audio_only = file("c.m4a", mp4_box(b"moov", &trak(b"soun", tkhd(0, 0, false))));
        assert_eq!(video_info(&audio_only), None);
        std::fs::remove_dir_all(&dir).ok();
    }
}
