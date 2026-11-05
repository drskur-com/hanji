//! Nemotron 인식기: gpui-kit `speech`의 [`SpeechRecognizer`]를 transcribe.cpp로 구현한다(ADR-0054).
//!
//! 녹음 한 번마다 작업 스레드 하나가 모델 세션을 들고 스트림에 소리를 넣는다. 말이 끊기면(글이 [`PAUSE_MS`]
//! 동안 바뀌지 않으면) 그때까지를 한 문단으로 맺어 쓰기 스레드([`Pipeline`])로 넘기고 새 스트림을 연다.
//! 문단마다 스트림을 새로 열어 오래 켜 두어도 스트림의 글이 한없이 자라지 않는다. 언어는 모델이 알아낸다.
//!
//! 기록 파일은 녹음을 시작하기 전에 정한다(새 기록이면 미리 만들고, 지난 기록이면 그 끝에 잇는다).
//! 멈추기를 기다리다 세션이 먼저 끝나도 남은 소리를 마저 인식해 저장하고, 쓰기 스레드까지 다 쓰면
//! [`Event::Done`](super::RecordEvent::Done)을 알린다.

use std::cell::{Cell, RefCell};
use std::fs::File;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc as std_mpsc;

use futures::channel::mpsc;
use gpui_kit::component::speech::{RecognitionSession, SpeechError, SpeechRecognizer, SpeechSink};
use gpui_kit::gpui::*;
#[cfg(feature = "dictation")]
use transcribe_cpp::{Model, RunOptions, StreamOptions};

use super::pipeline::{
    EventHandler, Pipeline, RecordingSetup, SAMPLE_RATE, WorkerEvent, forward_events,
};

/// 한 번에 스트림에 넣는 소리 길이(160 ms). 마이크는 더 잘게 보내 모아서 넣는다.
const FEED_SAMPLES: usize = SAMPLE_RATE * 160 / 1000;

/// 글이 이만큼(ms) 바뀌지 않으면 말이 끊긴 것으로 보고 문단을 맺는다.
const PAUSE_MS: i64 = 2_000;

/// 쉬지 않고 말해도 문단을 이 길이(ms)에서 맺는다. 말없이 켜 두었을 때도 이 길이마다 스트림을 새로 연다.
const MAX_PARAGRAPH_MS: i64 = 180_000;

/// Nemotron으로 받아쓰고 기록 파일 끝에 쓰는 인식기.
pub struct NemotronRecognizer {
    /// 모델 파일(GGUF).
    pub model: PathBuf,
    /// 이번 녹음의 설정. 녹음을 시작할 때 읽는다.
    pub setup: Rc<RefCell<RecordingSetup>>,
    /// 작업을 띄웠으면 `true`가 된다. 시작하다 실패했는지(그러면 `Done`이 오지 않는다) 본다.
    pub started: Rc<Cell<bool>>,
    pub on_event: EventHandler,
}

impl SpeechRecognizer for NemotronRecognizer {
    fn is_available(&self, _cx: &App) -> bool {
        self.model.is_file()
    }

    fn start(
        &self,
        sink: SpeechSink,
        cx: &mut App,
    ) -> Result<Box<dyn RecognitionSession>, SpeechError> {
        let (audio_tx, audio_rx) = std_mpsc::channel::<Vec<i16>>();
        let (event_tx, events) = mpsc::unbounded();
        let (model, setup) = (self.model.clone(), self.setup.borrow().clone());
        std::thread::Builder::new()
            .name("hanji-stt".into())
            .spawn(move || run(&model, setup, audio_rx, event_tx))
            .map_err(SpeechError::recognizer)?;
        forward_events(events, sink, self.on_event.clone(), &self.started, cx);
        Ok(Box::new(Session {
            audio: Some(audio_tx),
        }))
    }
}

/// 진행 중인 녹음. 소리를 작업 스레드로 넘긴다.
struct Session {
    audio: Option<std_mpsc::Sender<Vec<i16>>>,
}

impl RecognitionSession for Session {
    fn push_audio(&mut self, samples: &[i16], _cx: &mut App) {
        if let Some(audio) = &self.audio {
            let _ = audio.send(samples.to_vec());
        }
    }

    /// 소리 채널을 닫는다. 작업 스레드는 남은 소리를 인식해 마지막 문단을 쓰고 `Finished`를 보낸다.
    fn finish(&mut self, _cx: &mut App) {
        self.audio = None;
    }
}

/// 작업 스레드: 소리 채널이 닫힐 때까지 문단 단위로 인식해 쓰기 스레드로 넘기고, 쓰기 스레드가 다 쓰면 끝낸다.
/// `dictation` 기능 없이 빌드하면 인식기 없이 곧 실패를 알린다.
#[cfg(not(feature = "dictation"))]
fn run(
    _model: &Path,
    _setup: RecordingSetup,
    _audio: std_mpsc::Receiver<Vec<i16>>,
    events: mpsc::UnboundedSender<WorkerEvent>,
) {
    let _ = events.unbounded_send(WorkerEvent::Failed(super::NOT_BUILT.into()));
}

#[cfg(feature = "dictation")]
fn run(
    model: &Path,
    setup: RecordingSetup,
    audio: std_mpsc::Receiver<Vec<i16>>,
    events: mpsc::UnboundedSender<WorkerEvent>,
) {
    let send = |event| {
        let _ = events.unbounded_send(event);
    };
    let mut pipeline = None;
    let result = (|| {
        // 모델은 녹음마다 불러(메모리 매핑이라 금방이다) 녹음이 끝나면 놓는다. 앱이 끝날 때까지 들고 있으면
        // 종료 중 ggml Metal 장치를 정리하다 멈춘다(GGML_ASSERT).
        let model = Model::load(model).map_err(|err| err.to_string())?;
        let mut session = model.session().map_err(|err| err.to_string())?;
        send(WorkerEvent::Ready);
        let written = events.clone();
        let pipeline = pipeline.insert(Pipeline::start(setup, move |text| {
            let _ = written.unbounded_send(WorkerEvent::Written(text));
        })?);
        let run_options = RunOptions::default();
        let mut pending: Vec<f32> = Vec::with_capacity(FEED_SAMPLES * 2);
        let mut ended = false;
        while !ended {
            let mut stream = session
                .stream(&run_options, &StreamOptions::default())
                .map_err(|err| err.to_string())?;
            let mut shown = String::new();
            let mut changed_ms = 0;
            loop {
                match audio.recv() {
                    Ok(samples) => {
                        pipeline.audio(&samples);
                        pending.extend(samples.iter().map(|&s| f32::from(s) / 32768.0));
                    }
                    Err(_) => {
                        ended = true;
                        break;
                    }
                }
                if pending.len() < FEED_SAMPLES {
                    continue;
                }
                let update = stream.feed(&pending).map_err(|err| err.to_string())?;
                pending.clear();
                if update.committed_changed || update.tentative_changed {
                    changed_ms = update.input_received_ms;
                    let text = stream.text().display();
                    if text != shown {
                        send(WorkerEvent::Hypothesis(text.clone()));
                        shown = text;
                    }
                }
                let spoken = !shown.trim().is_empty();
                let paused = update.input_received_ms - changed_ms >= PAUSE_MS;
                if (spoken && paused) || update.input_received_ms >= MAX_PARAGRAPH_MS {
                    break;
                }
            }
            if ended && !pending.is_empty() {
                stream.feed(&pending).map_err(|err| err.to_string())?;
                pending.clear();
            }
            stream.finalize().map_err(|err| err.to_string())?;
            let text = stream.text().full;
            drop(stream);
            // 쓰기 스레드가 이미 끝났으면(쓰기 실패) 아래에서 그 까닭을 알린다.
            if !pipeline.cut(text) {
                break;
            }
            send(WorkerEvent::Hypothesis(String::new()));
        }
        Ok::<_, String>(())
    })();
    // 쓰기 스레드가 남은 문단을 다 쓸 때까지 기다린다.
    let written = pipeline.map_or(Ok(()), Pipeline::finish);
    send(match result.and(written) {
        Ok(()) => WorkerEvent::Finished,
        Err(reason) => WorkerEvent::Failed(reason),
    });
}

/// 기록의 제목 줄(`# 받아쓰기 YYYY-MM-DD HH:MM`, 지금 시각).
fn title_line() -> String {
    let started = chrono::Local::now().format("%Y-%m-%d %H:%M");
    format!("# {}\n", t!("dictation.file_title", time = started))
}

/// 새 기록 파일(`<dir>/<ULID>.md`)을 제목 줄과 함께 만든다.
pub fn create_record(dir: &Path) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.md", ulid::Ulid::generate()));
    let mut file = File::options().create_new(true).write(true).open(&path)?;
    file.write_all(title_line().as_bytes())?;
    Ok(path)
}

/// 비어 있는 기록(시트 목록의 작성 버튼으로 만든 시트)에 녹음하기 전에 제목 줄을 넣는다. 넣었으면 `true`.
/// 제목이 없으면 첫 문단이 기록 이름이 되고, 요약을 넣으면 "요약"이 이름이 된다.
pub fn ensure_record_title(path: &Path) -> io::Result<bool> {
    if !std::fs::read_to_string(path)?.trim().is_empty() {
        return Ok(false);
    }
    std::fs::write(path, title_line())?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    // `super::*`는 gpui의 `test` 매크로까지 들여와 `#[test]`를 가린다.
    use super::{SAMPLE_RATE, WorkerEvent, create_record, ensure_record_title, run};
    use crate::dictation::pipeline::{RecordingSetup, Refiner};
    use futures::StreamExt as _;
    use std::path::PathBuf;

    #[test]
    fn create_record_writes_title_line() {
        let dir = std::env::temp_dir().join(format!("hanji-stt-{}", std::process::id()));
        let path = create_record(&dir).unwrap();
        assert!(path.extension().is_some_and(|ext| ext == "md"));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# ") && text.ends_with('\n') && text.lines().count() == 1);
        // 제목이 있으면 그대로, 비었으면 제목 줄을 넣는다.
        assert!(!ensure_record_title(&path).unwrap());
        std::fs::write(&path, " \n").unwrap();
        assert!(ensure_record_title(&path).unwrap());
        assert!(std::fs::read_to_string(&path).unwrap().starts_with("# "));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// WAV(16 kHz 모노 16비트)의 소리.
    fn read_wav(path: &str) -> Vec<i16> {
        let bytes = std::fs::read(path).unwrap();
        let data = bytes.windows(4).position(|w| w == b"data").unwrap() + 8;
        bytes[data..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&b| i16::from_le_bytes(b))
            .collect()
    }

    /// `WRITER_STT_COHERE`가 있으면 Cohere(언어는 `WRITER_STT_LANGUAGE` 또는 `ko`).
    fn refiner_from_env() -> Option<Refiner> {
        std::env::var("WRITER_STT_COHERE")
            .ok()
            .map(|cohere| Refiner {
                model: cohere.into(),
                language: std::env::var("WRITER_STT_LANGUAGE").unwrap_or_else(|_| "ko".into()),
            })
    }

    /// 소리를 마이크처럼 10 ms씩 넣고, 쓴 문단과 기록 파일의 글을 돌려준다.
    fn replay(model: &str, refiner: Option<Refiner>, speech: &[i16]) -> (Vec<String>, String) {
        let dir = std::env::temp_dir().join(format!(
            "hanji-stt-real-{}-{}",
            std::process::id(),
            refiner.is_some()
        ));
        let target = create_record(&dir).unwrap();
        let setup = RecordingSetup {
            target: target.clone(),
            refiner,
            audio_file: None,
        };
        let (audio_tx, audio_rx) = std::sync::mpsc::channel();
        let (event_tx, events) = futures::channel::mpsc::unbounded();
        let worker = {
            let model = PathBuf::from(model);
            std::thread::spawn(move || run(&model, setup, audio_rx, event_tx))
        };
        for chunk in speech.chunks(160) {
            audio_tx.send(chunk.to_vec()).unwrap();
        }
        drop(audio_tx);
        worker.join().unwrap();
        let events: Vec<_> = futures::executor::block_on(events.collect());
        let paragraphs: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                WorkerEvent::Written(text) => Some(text.clone()),
                WorkerEvent::Failed(reason) => panic!("{reason}"),
                _ => None,
            })
            .collect();
        assert!(matches!(events.last(), Some(WorkerEvent::Finished)));
        let text = std::fs::read_to_string(&target).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
        for paragraph in &paragraphs {
            eprintln!("{paragraph}\n");
        }
        (paragraphs, text)
    }

    /// 실제 모델로 말 → 3초 쉼 → 말 → 3초 쉼을 넣으면 문단 두 개가 파일에 쌓인다. `WRITER_STT_COHERE`를 주면
    /// 문단을 Cohere로 다시 받아 적되 문단은 Nemotron이 나눈 그대로 두 개다.
    /// `WRITER_STT_MODEL`(GGUF)과 `WRITER_STT_WAV`(16 kHz 모노 16비트)를 주고 `--ignored`로 돌린다.
    /// 한국어 사이에 영어 낱말이 섞인 소리(`say -v Yuna "… roadmap을 …"`)로 견주어 볼 수 있다.
    #[test]
    #[ignore]
    fn pause_splits_paragraphs_with_real_model() {
        let (Ok(model), Ok(wav)) = (
            std::env::var("WRITER_STT_MODEL"),
            std::env::var("WRITER_STT_WAV"),
        ) else {
            return;
        };
        let speech = read_wav(&wav);
        let silence = vec![0i16; SAMPLE_RATE * 3];
        let twice = [&speech[..], &silence, &speech, &silence].concat();
        let (paragraphs, text) = replay(&model, refiner_from_env(), &twice);
        assert_eq!(paragraphs.len(), 2);
        // 제목, 빈 줄, 문단, 빈 줄, 문단.
        assert_eq!(text.lines().count(), 5);
    }

    /// 저장한 녹음(`STT/.audio/*.wav`, `WRITER_STT_WAV`)을 한 번 그대로 넣고 문단을 찍는다. 모델을 견줄 때 쓴다.
    /// `cargo test --release replays_saved_recording -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn replays_saved_recording() {
        let (Ok(model), Ok(wav)) = (
            std::env::var("WRITER_STT_MODEL"),
            std::env::var("WRITER_STT_WAV"),
        ) else {
            return;
        };
        replay(&model, refiner_from_env(), &read_wav(&wav));
    }

    /// 긴 녹음(`WRITER_STT_WAV`)을 넣으며 자막(`Hypothesis`) 이벤트의 벽시계 시각을 찍는다. 실시간보다
    /// 늦어지는지(자막이 멈추는 까닭) 본다. `--ignored --nocapture`로 돌린다.
    #[test]
    #[ignore]
    fn hypothesis_timing_on_long_recording() {
        let (Ok(model), Ok(wav)) = (
            std::env::var("WRITER_STT_MODEL"),
            std::env::var("WRITER_STT_WAV"),
        ) else {
            return;
        };
        let speech = read_wav(&wav);
        let total = speech.len() as f64 / SAMPLE_RATE as f64;
        // Windows는 백엔드를 실행 중에 불러오는 빌드라 모델을 부르기 전에 등록해야 한다(앱 main이 하는 일).
        super::super::init_backends();
        let dir = std::env::temp_dir().join(format!("hanji-stt-timing-{}", std::process::id()));
        let target = create_record(&dir).unwrap();
        let setup = RecordingSetup {
            target,
            refiner: None,
            audio_file: None,
        };
        let (audio_tx, audio_rx) = std::sync::mpsc::channel();
        let (event_tx, events) = futures::channel::mpsc::unbounded();
        std::thread::spawn(move || run(&PathBuf::from(model), setup, audio_rx, event_tx));
        let started = std::time::Instant::now();
        let mut sent_ms = 0;
        for chunk in speech.chunks(160) {
            audio_tx.send(chunk.to_vec()).unwrap();
            sent_ms += 10;
            // 벽시계로 실시간을 흉내 낸다(진짜 마이크처럼). 녹음 길이만큼 걸린다.
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        drop(audio_tx);
        let events: Vec<_> = futures::executor::block_on(events.collect());
        println!(
            "음성 길이 {total:.1}초, 재생 벽시계 {:.1}초",
            started.elapsed().as_secs_f64()
        );
        let mut printed = 0;
        for event in &events {
            if let WorkerEvent::Hypothesis(text) = event {
                printed += 1;
                if printed % 20 == 0 || text.is_empty() {
                    println!(
                        "[자막 갱신] 벽시계 {:.1}초 길이 {} 글자",
                        started.elapsed().as_secs_f64(),
                        text.chars().count()
                    );
                }
            }
        }
        println!("자막 갱신 {printed}회");
        for event in &events {
            if let WorkerEvent::Failed(reason) = event {
                println!("실패: {reason}");
            }
            if let WorkerEvent::Written(text) = event {
                println!(
                    "문단({}자): {}",
                    text.chars().count(),
                    &text[..text.len().min(120)]
                );
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
