//! 녹음 한 번의 뒤쪽 경로(ADR-0054, ADR-0056): 인식기가 넣은 소리와 문단 끝을 받아 소리를 남기고(WAV),
//! 문단을 Cohere로 다시 받아 적고(켰으면), 기록 끝에 쓴다. Nemotron·macOS 받아쓰기가 함께 쓴다.
//!
//! 인식기는 자기에게 넣은 소리를 같은 차례로 [`Pipeline::audio`]에 넘기고, 문단을 맺으면 그 글을
//! [`Pipeline::cut`]으로 넘긴다. 그래서 문단의 소리는 그 문단 동안 인식기에 넣은 소리와 꼭 같다.
//! 이 모든 일은 쓰기 스레드 하나가 하고(Cohere가 도는 동안에도 인식기는 기다리지 않는다), 기록 파일에는
//! 이 스레드만 쓴다.

use std::cell::Cell;
use std::io;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc as std_mpsc;

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::component::speech::{SpeechError, SpeechSink};
use gpui_kit::gpui::*;
use transcribe_cpp::{Model, RunOptions, Session as AsrSession};

use super::output::{Record, write_out};
use super::wav::WavWriter;

/// 인식기가 받는 소리: 16 kHz 모노(gpui-kit `AudioFormat`의 기본값과 같다).
pub(super) const SAMPLE_RATE: usize = 16_000;

/// Cohere에 넘기는 소리의 앞뒤에 붙이는 무음(0.5초). 없으면 첫 마디를 빠뜨리곤 한다.
const REFINE_PADDING: usize = SAMPLE_RATE / 2;

/// Cohere에 한 번에 넘기는 소리 길이의 상한(8초). 길게 넘기면 낱말이 흐트러지고 같은 말을 되풀이한다
/// (저장한 녹음으로 5·8·10·12·15·25초를 견주어 8초가 가장 나았다).
const REFINE_CHUNK: usize = SAMPLE_RATE * 8;

/// 조각을 자를 곳을 찾는 구간: 조각 끝 5초 안에서 가장 조용한 100 ms의 가운데에서 자른다.
const REFINE_SEARCH: usize = SAMPLE_RATE * 5;
const REFINE_WINDOW: usize = SAMPLE_RATE / 10;

/// 받아쓰기를 쓰는 쪽에 알리는 일.
pub enum Event {
    /// 파일 끝에 받아 적은 문단을 한 줄로 덧붙였다.
    Written(SharedString),
    /// 녹음이 끝났다. 이제 기록 파일에 더 쓰지 않는다.
    Done,
}

/// [`Event`]를 받는 쪽.
pub type EventHandler = Rc<dyn Fn(Event, &mut App)>;

/// 이번 녹음의 설정. 앱이 녹음을 시작하기 전에 채우고, 인식기가 시작할 때 읽는다.
#[derive(Clone, Debug, Default)]
pub struct RecordingSetup {
    /// 기록 파일.
    pub target: PathBuf,
    /// 문단을 다시 받아 적을 Cohere(ADR-0056).
    pub refiner: Option<Refiner>,
    /// 소리를 남길 WAV 파일. 없으면 남기지 않는다.
    pub audio_file: Option<PathBuf>,
}

/// 문단을 다시 받아 적는 Cohere 모델과 언어(ADR-0056).
#[derive(Clone, Debug)]
pub struct Refiner {
    /// 모델 파일(GGUF).
    pub model: PathBuf,
    /// 받아쓰는 언어(`DICTATION_LANGUAGES`의 코드).
    pub language: String,
}

/// 인식기의 작업이 메인 스레드로 보내는 일.
pub(super) enum WorkerEvent {
    Ready,
    Hypothesis(String),
    Written(String),
    Failed(String),
    Finished,
}

/// 작업의 소식을 메인 스레드에서 녹음 상태(`sink`)와 앱(`on_event`)에 넘긴다. 세션보다 오래 산다:
/// 멈추기를 기다리다 세션이 끝나도 남은 문단과 `Done`을 넘긴다.
pub(super) fn forward_events(
    mut events: mpsc::UnboundedReceiver<WorkerEvent>,
    sink: SpeechSink,
    on_event: EventHandler,
    started: &Rc<Cell<bool>>,
    cx: &mut App,
) {
    started.set(true);
    cx.spawn(async move |cx| {
        while let Some(event) = events.next().await {
            let finished = matches!(event, WorkerEvent::Finished | WorkerEvent::Failed(_));
            cx.update(|cx| match event {
                WorkerEvent::Ready => sink.ready(cx),
                WorkerEvent::Hypothesis(text) => sink.hypothesis(text, cx),
                WorkerEvent::Written(text) => on_event(Event::Written(text.into()), cx),
                // 녹음 상태가 `SpeechEvent::Error`로 알린다.
                WorkerEvent::Failed(reason) => {
                    sink.error(SpeechError::recognizer(io::Error::other(reason)), cx);
                    on_event(Event::Done, cx);
                }
                WorkerEvent::Finished => {
                    sink.finish(cx);
                    on_event(Event::Done, cx);
                }
            });
            if finished {
                break;
            }
        }
    })
    .detach();
}

/// 쓰기 스레드로 보내는 것.
enum Piece {
    /// 인식기에 넣은 소리.
    Audio(Vec<i16>),
    /// 문단이 끝났다: 그 글(비었으면 문단 없이 소리만 버린다).
    Cut(String),
}

/// 녹음 한 번의 쓰기 스레드.
pub(super) struct Pipeline {
    pieces: std_mpsc::Sender<Piece>,
    thread: std::thread::JoinHandle<io::Result<()>>,
}

impl Pipeline {
    /// 기록을 열고 쓰기 스레드를 띄운다. 쓴 문단마다 `written`을 부른다(쓰기 스레드에서).
    pub(super) fn start(
        setup: RecordingSetup,
        written: impl FnMut(String) + Send + 'static,
    ) -> Result<Self, String> {
        let record = Record::open(&setup.target).map_err(|err| err.to_string())?;
        let (pieces, rx) = std_mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("hanji-stt-out".into())
            .spawn(move || write_pieces(record, setup, rx, written))
            .map_err(|err| err.to_string())?;
        Ok(Self { pieces, thread })
    }

    /// 인식기에 넣은 소리를 넘긴다.
    pub(super) fn audio(&self, samples: &[i16]) {
        let _ = self.pieces.send(Piece::Audio(samples.to_vec()));
    }

    /// 문단을 맺는다. 쓰기 스레드가 이미 끝났으면(쓰기 실패) `false`.
    pub(super) fn cut(&self, text: String) -> bool {
        self.pieces.send(Piece::Cut(text)).is_ok()
    }

    /// 남은 문단을 다 쓸 때까지 기다린다. 쓰기에 실패했으면 그 까닭.
    pub(super) fn finish(self) -> Result<(), String> {
        drop(self.pieces);
        match self.thread.join() {
            Ok(result) => result.map_err(|err| err.to_string()),
            Err(_) => Err("record writer panicked".to_string()),
        }
    }
}

/// 쓰기 스레드: 소리를 남기고 모으다가 문단 끝마다 (Cohere로 다시 받아 적어) 기록에 쓴다.
fn write_pieces(
    record: Record,
    setup: RecordingSetup,
    pieces: std_mpsc::Receiver<Piece>,
    written: impl FnMut(String),
) -> io::Result<()> {
    // 소리 파일을 열지 못해도, Cohere를 불러오지 못해도 받아쓰기는 이어 간다.
    let mut wav = setup.audio_file.and_then(|path| {
        WavWriter::open(&path)
            .inspect_err(|err| eprintln!("녹음 소리 파일을 열지 못했습니다: {err}"))
            .ok()
    });
    let mut refining = setup.refiner.and_then(|refiner| {
        Refining::load(&refiner)
            .inspect_err(|err| eprintln!("Cohere를 불러오지 못했습니다: {err}"))
            .ok()
    });
    let mut audio: Vec<f32> = Vec::new();
    let paragraphs = pieces.into_iter().filter_map(|piece| match piece {
        Piece::Audio(samples) => {
            if let Some(file) = &mut wav
                && let Err(err) = file.write(&samples)
            {
                eprintln!("녹음 소리를 쓰지 못했습니다: {err}");
                wav = None;
            }
            if refining.is_some() {
                audio.extend(samples.iter().map(|&s| f32::from(s) / 32768.0));
            }
            None
        }
        Piece::Cut(text) => {
            let audio = std::mem::take(&mut audio);
            let text = text.trim().to_string();
            if text.is_empty() {
                return None;
            }
            Some(match &mut refining {
                Some(refining) => refining.transcribe(text, &audio),
                None => text,
            })
        }
    });
    let result = write_out(record, paragraphs, written);
    if let Some(Err(err)) = wav.map(WavWriter::finish) {
        eprintln!("녹음 소리 파일을 닫지 못했습니다: {err}");
    }
    result
}

/// 쓰기 스레드에서 문단을 Cohere로 다시 받아 적는다.
struct Refining {
    session: AsrSession,
    options: RunOptions,
}

impl Refining {
    /// 모델은 녹음마다 불러(메모리 매핑) 녹음이 끝나면 놓는다(ADR-0054: 종료 중 ggml Metal 정리).
    fn load(refiner: &Refiner) -> Result<Self, String> {
        let model = Model::load(&refiner.model).map_err(|err| err.to_string())?;
        Ok(Self {
            session: model.session().map_err(|err| err.to_string())?,
            options: RunOptions {
                language: Some(refiner.language.clone()),
                ..Default::default()
            },
        })
    }

    /// 다시 받아 적은 글. 문단 소리를 [`REFINE_CHUNK`] 안쪽 조각으로 나눠 받아 적고 잇는다. 한 조각이라도
    /// 실패하거나 모두 빈 글이면 인식기의 글(`text`).
    fn transcribe(&mut self, text: String, audio: &[f32]) -> String {
        let mut parts = Vec::new();
        for chunk in split_quiet(audio) {
            let mut pcm = vec![0.0; REFINE_PADDING];
            pcm.extend_from_slice(chunk);
            pcm.resize(pcm.len() + REFINE_PADDING, 0.0);
            match self.session.run(&pcm, &self.options) {
                Ok(transcript) => parts.push(transcript.text.trim().to_string()),
                Err(err) => {
                    eprintln!("Cohere로 다시 받아 적지 못했습니다: {err}");
                    return text;
                }
            }
        }
        let refined = collapse_repeats(&parts.join(" "));
        if refined.is_empty() { text } else { refined }
    }
}

/// 소리를 [`REFINE_CHUNK`] 안쪽 조각으로 나눈다. 말 가운데를 자르지 않게 조각 끝 [`REFINE_SEARCH`] 안에서
/// 가장 조용한 곳에서 자른다.
fn split_quiet(audio: &[f32]) -> Vec<&[f32]> {
    let mut chunks = Vec::new();
    let mut rest = audio;
    while rest.len() > REFINE_CHUNK {
        let from = REFINE_CHUNK - REFINE_SEARCH;
        let energy = |at: usize| {
            rest[at..at + REFINE_WINDOW]
                .iter()
                .map(|x| x * x)
                .sum::<f32>()
        };
        let quiet = (from..=REFINE_CHUNK - REFINE_WINDOW)
            .step_by(REFINE_WINDOW / 10)
            .min_by(|&a, &b| energy(a).total_cmp(&energy(b)))
            .unwrap_or(from);
        let (chunk, next) = rest.split_at(quiet + REFINE_WINDOW / 2);
        chunks.push(chunk);
        rest = next;
    }
    if !rest.is_empty() {
        chunks.push(rest);
    }
    chunks
}

/// 같은 말(낱말 1–6개)이 세 번 넘게 잇달아 나오면 한 번만 남긴다. 모델이 잡음·음악에서 같은 말을
/// 되풀이하곤 한다("나도 모르겠어요. 나도 모르겠어요. …", "참, 참, 참, …").
fn collapse_repeats(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let key = |word: &str| {
        word.trim_matches(|c: char| !c.is_alphanumeric())
            .to_lowercase()
    };
    let mut out: Vec<&str> = Vec::new();
    let mut i = 0;
    'words: while i < words.len() {
        for n in 1..=6 {
            let repeats = (1..)
                .take_while(|r| {
                    let at = i + r * n;
                    at + n <= words.len() && (0..n).all(|k| key(words[i + k]) == key(words[at + k]))
                })
                .count();
            if repeats >= 2 {
                out.extend(&words[i..i + n]);
                i += (repeats + 1) * n;
                continue 'words;
            }
        }
        out.push(words[i]);
        i += 1;
    }
    out.join(" ")
}

#[cfg(test)]
mod tests {
    // `super::*`는 gpui의 `test` 매크로까지 들여와 `#[test]`를 가린다.
    use super::{
        Pipeline, REFINE_CHUNK, RecordingSetup, SAMPLE_RATE, collapse_repeats, split_quiet,
        std_mpsc,
    };

    #[test]
    fn splits_long_audio_at_quiet_points() {
        assert_eq!(split_quiet(&[]).len(), 0);
        assert_eq!(split_quiet(&vec![0.5; REFINE_CHUNK]).len(), 1);
        // 큰 소리 사이 6.5초에 쉼이 있으면 그 가운데에서 자른다.
        let mut audio = vec![0.5; SAMPLE_RATE * 12];
        let pause = SAMPLE_RATE * 13 / 2;
        audio[pause..pause + SAMPLE_RATE / 5].fill(0.0);
        let chunks = split_quiet(&audio);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].len() > pause && chunks[0].len() < pause + SAMPLE_RATE / 5);
        assert_eq!(chunks.iter().map(|c| c.len()).sum::<usize>(), audio.len());
        assert!(
            split_quiet(&vec![0.5; SAMPLE_RATE * 30])
                .iter()
                .all(|c| c.len() <= REFINE_CHUNK)
        );
    }

    #[test]
    fn collapses_repeated_phrases() {
        assert_eq!(
            collapse_repeats("나도 모르겠어요. 나도 모르겠어요. 나도 모르겠어요. 그만."),
            "나도 모르겠어요. 그만."
        );
        assert_eq!(collapse_repeats("왜, 참, 참, 참, 참."), "왜, 참,");
        // 두 번까지는 그대로 둔다.
        assert_eq!(collapse_repeats("네 네 알겠습니다"), "네 네 알겠습니다");
    }

    /// 문단의 소리·글이 차례대로 쓰이고, 빈 문단은 소리만 버리며, 소리 파일에는 모든 소리가 남는다.
    #[test]
    fn writes_cut_paragraphs_and_keeps_audio() {
        let dir = std::env::temp_dir().join(format!("hanji-pipeline-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("a.md");
        let audio_file = dir.join(".audio").join("a.wav");
        let (tx, rx) = std_mpsc::channel();
        let pipeline = Pipeline::start(
            RecordingSetup {
                target: target.clone(),
                refiner: None,
                audio_file: Some(audio_file.clone()),
            },
            move |text| tx.send(text).unwrap(),
        )
        .unwrap();
        pipeline.audio(&[1, 2]);
        assert!(pipeline.cut(" 하나 ".into()));
        pipeline.audio(&[3]);
        assert!(pipeline.cut(String::new()));
        assert!(pipeline.cut("둘".into()));
        pipeline.finish().unwrap();
        assert_eq!(rx.into_iter().collect::<Vec<_>>(), ["하나", "둘"]);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "하나\n\n둘\n");
        assert_eq!(std::fs::read(&audio_file).unwrap().len(), 44 + 6);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
