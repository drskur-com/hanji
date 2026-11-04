//! 녹음 소리 파일(ADR-0054): 인식기에 넣는 소리(16 kHz 모노 16비트)를 WAV로 쓴다.
//!
//! 기록마다 `<기록 폴더>/.audio/<기록 파일 이름>.wav` 하나다. 지난 기록 끝에 이어서 녹음하면 그 파일 끝에
//! 잇는다. 머리(44바이트)의 크기는 닫을 때 맞추고, 앱이 죽어 맞추지 못했어도 다음에 열 때 파일 길이로 맞춘다.

use std::fs::File;
use std::io::{self, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};

/// 소리 형식: 16 kHz 모노 16비트.
const SAMPLE_RATE: u32 = 16_000;
const HEADER_LEN: u64 = 44;

/// 기록의 소리 파일(`<기록 폴더>/.audio/<기록 파일 이름>.wav`). 점 폴더라 라이브러리에 잡히지 않는다.
pub fn audio_path(record: &Path) -> Option<PathBuf> {
    let name = record.file_stem()?.to_string_lossy();
    Some(record.parent()?.join(".audio").join(format!("{name}.wav")))
}

/// 쓰는 중인 소리 파일.
pub(super) struct WavWriter {
    file: File,
}

impl WavWriter {
    /// 소리 파일을 연다. 이 앱이 쓴 파일이면 끝에 잇고, 없거나 형식이 다르면 새로 쓴다.
    pub(super) fn open(path: &Path) -> io::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        let mut header = [0u8; HEADER_LEN as usize];
        let ours = std::io::Read::read_exact(&mut file, &mut header).is_ok()
            && header
                == wav_header(u32::from_le_bytes([
                    header[40], header[41], header[42], header[43],
                ]));
        if !ours {
            file.set_len(0)?;
            file.seek(SeekFrom::Start(0))?;
            file.write_all(&wav_header(0))?;
        }
        file.seek(SeekFrom::End(0))?;
        Ok(Self { file })
    }

    pub(super) fn write(&mut self, samples: &[i16]) -> io::Result<()> {
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        self.file.write_all(&bytes)
    }

    /// 머리의 크기를 파일 길이에 맞추고 닫는다.
    pub(super) fn finish(mut self) -> io::Result<()> {
        let len = self.file.seek(SeekFrom::End(0))?;
        let data = u32::try_from(len.saturating_sub(HEADER_LEN)).unwrap_or(u32::MAX);
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(&wav_header(data))?;
        self.file.flush()
    }
}

/// `data`바이트의 소리가 뒤따르는 WAV 머리.
fn wav_header(data: u32) -> [u8; HEADER_LEN as usize] {
    let mut header = [0u8; HEADER_LEN as usize];
    let mut put = |at: usize, bytes: &[u8]| header[at..at + bytes.len()].copy_from_slice(bytes);
    put(0, b"RIFF");
    put(4, &data.saturating_add(36).to_le_bytes());
    put(8, b"WAVEfmt ");
    put(16, &16u32.to_le_bytes());
    put(20, &1u16.to_le_bytes()); // PCM
    put(22, &1u16.to_le_bytes()); // 모노
    put(24, &SAMPLE_RATE.to_le_bytes());
    put(28, &(SAMPLE_RATE * 2).to_le_bytes());
    put(32, &2u16.to_le_bytes());
    put(34, &16u16.to_le_bytes());
    put(36, b"data");
    put(40, &data.to_le_bytes());
    header
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_continues_wav() {
        let dir = std::env::temp_dir().join(format!("hanji-wav-{}", std::process::id()));
        let path = audio_path(&dir.join("01J.md")).unwrap();
        assert_eq!(path, dir.join(".audio").join("01J.wav"));
        let mut wav = WavWriter::open(&path).unwrap();
        wav.write(&[1, 2, 3]).unwrap();
        wav.finish().unwrap();
        // 이어서 녹음하면 끝에 잇는다.
        let mut wav = WavWriter::open(&path).unwrap();
        wav.write(&[4]).unwrap();
        wav.finish().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(bytes.len(), 44 + 8);
        assert_eq!(&bytes[..44], &wav_header(8));
        assert_eq!(&bytes[44..], &[1, 0, 2, 0, 3, 0, 4, 0]);
        // 우리 파일이 아니면 새로 쓴다.
        std::fs::write(
            &path,
            b"not a wav file at all, but long enough to read a header",
        )
        .unwrap();
        WavWriter::open(&path).unwrap().finish().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), wav_header(0));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
