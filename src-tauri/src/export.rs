//! Transcript rendering — breadcrumbs for the editor, never an editing tool.

use crate::store::TranscriptSegment;

/// Format milliseconds as `HH:MM:SS,mmm` (SRT timestamp style).
fn fmt_ms(ms: i64) -> String {
    let hours = ms / 3_600_000;
    let minutes = (ms % 3_600_000) / 60_000;
    let seconds = (ms % 60_000) / 1_000;
    let millis = ms % 1_000;
    format!("{hours:02}:{minutes:02}:{seconds:02},{millis:03}")
}

/// Render segments as SubRip (.srt): 1-based index, timestamp range, text,
/// blank line between entries.
pub fn to_srt(segments: &[TranscriptSegment]) -> String {
    let mut out = String::new();
    for (i, seg) in segments.iter().enumerate() {
        out.push_str(&format!(
            "{}\n{} --> {}\n{}\n\n",
            i + 1,
            fmt_ms(seg.start_ms),
            fmt_ms(seg.end_ms),
            seg.text
        ));
    }
    out
}

/// Render segments as plain text: one line of text per segment.
pub fn to_txt(segments: &[TranscriptSegment]) -> String {
    let mut out = String::new();
    for seg in segments {
        out.push_str(&seg.text);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::TranscriptSegment;

    fn seg(start: i64, end: i64, text: &str) -> TranscriptSegment {
        TranscriptSegment { id: 0, session_id: 1, start_ms: start, end_ms: end, text: text.into() }
    }

    #[test]
    fn renders_srt_with_timestamps() {
        let srt = to_srt(&[seg(0, 1500, "hello world"), seg(61_020, 62_000, "a minute in")]);
        assert!(srt.contains("1\n00:00:00,000 --> 00:00:01,500\nhello world"));
        assert!(srt.contains("2\n00:01:01,020 --> 00:01:02,000\na minute in"));
    }

    #[test]
    fn renders_plain_text_lines() {
        let txt = to_txt(&[seg(0, 1000, "one"), seg(1000, 2000, "two")]);
        assert_eq!(txt, "one\ntwo\n");
    }
}
