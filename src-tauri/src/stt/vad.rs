//! Cuts a continuous 16 kHz stream into utterances on energy dips. This is
//! the fast-lane's first cousin: cheap, no ML, tuned conservative. False
//! splits are fine (the LLM lane later re-joins meaning); missed splits are
//! bounded by the force-cut.

const RATE: usize = 16_000;
const FRAME_MS: usize = 30;
const FRAME: usize = RATE * FRAME_MS / 1000;
const SILENCE_RMS: f32 = 0.01;
const END_SILENCE_MS: usize = 600;
const MIN_SPEECH_MS: usize = 250;
pub const MAX_UTTERANCE_MS: usize = 12_000;

pub struct Utterance {
    pub start_ms: i64,
    pub samples: Vec<f32>,
}

pub struct UtteranceChunker {
    buf: Vec<f32>,      // frames since utterance start (incl. leading silence trim)
    stream_pos_ms: i64, // absolute position in the 16k stream
    utter_start_ms: Option<i64>,
    silence_run_ms: usize,
    speech_ms: usize,
    pending: Vec<f32>, // partial frame carry-over
}

impl Default for UtteranceChunker {
    fn default() -> Self {
        Self::new()
    }
}

impl UtteranceChunker {
    pub fn new() -> Self {
        Self {
            buf: Vec::new(),
            stream_pos_ms: 0,
            utter_start_ms: None,
            silence_run_ms: 0,
            speech_ms: 0,
            pending: Vec::new(),
        }
    }

    pub fn push(&mut self, samples_16k: &[f32]) -> Vec<Utterance> {
        let mut out = Vec::new();
        self.pending.extend_from_slice(samples_16k);
        while self.pending.len() >= FRAME {
            let frame: Vec<f32> = self.pending.drain(..FRAME).collect();
            let rms = (frame.iter().map(|s| s * s).sum::<f32>() / FRAME as f32).sqrt();
            let is_speech = rms > SILENCE_RMS;

            if self.utter_start_ms.is_none() {
                if is_speech {
                    self.utter_start_ms = Some(self.stream_pos_ms);
                    self.buf.extend_from_slice(&frame);
                    self.speech_ms = FRAME_MS;
                    self.silence_run_ms = 0;
                }
            } else {
                self.buf.extend_from_slice(&frame);
                if is_speech {
                    self.speech_ms += FRAME_MS;
                    self.silence_run_ms = 0;
                } else {
                    self.silence_run_ms += FRAME_MS;
                }
                let dur_ms = self.buf.len() * 1000 / RATE;
                if self.silence_run_ms >= END_SILENCE_MS || dur_ms >= MAX_UTTERANCE_MS {
                    if let Some(u) = self.take_utterance() {
                        out.push(u);
                    }
                }
            }
            self.stream_pos_ms += FRAME_MS as i64;
        }
        out
    }

    /// End-of-session: hand back whatever is mid-flight.
    pub fn flush(&mut self) -> Option<Utterance> {
        self.take_utterance()
    }

    fn take_utterance(&mut self) -> Option<Utterance> {
        let start_ms = self.utter_start_ms.take()?;
        let samples = std::mem::take(&mut self.buf);
        let had_speech = self.speech_ms >= MIN_SPEECH_MS;
        self.speech_ms = 0;
        self.silence_run_ms = 0;
        had_speech.then_some(Utterance { start_ms, samples })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loud(ms: usize) -> Vec<f32> {
        vec![0.3; 16 * ms]
    } // 16 samples/ms @16k
    fn quiet(ms: usize) -> Vec<f32> {
        vec![0.001; 16 * ms]
    }

    #[test]
    fn emits_utterance_after_trailing_silence() {
        let mut vad = UtteranceChunker::new();
        let mut got = Vec::new();
        got.extend(vad.push(&loud(1000)));
        got.extend(vad.push(&quiet(300)));
        assert!(got.is_empty(), "should still be waiting for enough silence");
        got.extend(vad.push(&quiet(500)));
        assert_eq!(got.len(), 1);
        // Utterance contains the loud second (plus some silence padding).
        assert!(got[0].samples.len() >= 16 * 900);
        assert_eq!(got[0].start_ms, 0);
    }

    #[test]
    fn pure_silence_emits_nothing() {
        let mut vad = UtteranceChunker::new();
        assert!(vad.push(&quiet(5000)).is_empty());
    }

    #[test]
    fn force_cuts_overlong_utterances() {
        let mut vad = UtteranceChunker::new();
        let got = vad.push(&loud(13_000)); // no pause for 13s
        assert_eq!(got.len(), 1, "must force-cut at MAX_UTTERANCE_MS");
    }

    #[test]
    fn flush_returns_tail_in_progress() {
        let mut vad = UtteranceChunker::new();
        assert!(vad.push(&loud(700)).is_empty());
        let tail = vad.flush();
        assert!(tail.is_some());
    }
}
