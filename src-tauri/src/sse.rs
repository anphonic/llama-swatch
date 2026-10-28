//! Minimal Server-Sent Events parser.
//!
//! Feed raw bytes as they arrive from the network. Bytes are buffered until a
//! full line is available, so chunks may split lines or even UTF-8 code points.
//! Only the `data` field matters to us; `event`, `id`, `retry` and comments are ignored.

/// Longest single line we buffer; a server that never sends `\n` cannot grow memory past this.
/// The count includes a trailing `\r`.
pub const MAX_LINE_BYTES: usize = 1024 * 1024;
/// Most `data` bytes one event may hold, counting the `\n` between its lines, before it is dropped.
pub const MAX_EVENT_BYTES: usize = 4 * 1024 * 1024;

/// What one event became, in stream order.
#[derive(Debug, PartialEq, Eq)]
pub enum Frame {
    /// The event's `data` lines joined with `\n`.
    Data(String),
    /// The event was dropped for size. A dropped event yields exactly one of these.
    Dropped,
}

#[derive(Debug, Default)]
pub struct SseParser {
    buf: Vec<u8>,
    /// Prefix of `buf` already known to hold no `\n`, so each byte is scanned once.
    scanned: usize,
    /// The current event's `data` lines, already joined, so the cap counts every byte held:
    /// an empty `data:` line still costs its separator.
    data: String,
    /// Whether the current event has a `data` line; `data:` alone is an event with empty data.
    has_data: bool,
    /// Discarding the rest of an over-long line up to its next `\n`.
    skipping_line: bool,
    /// The current event lost a line or grew too big; drop it at its blank line.
    drop_event: bool,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(test)]
    fn buffered_len(&self) -> usize {
        self.buf.len()
    }

    /// Drops the current event, reporting it the first time only.
    fn overflow(&mut self, out: &mut Vec<Frame>) {
        if !self.drop_event {
            out.push(Frame::Dropped);
        }
        self.drop_event = true;
        self.data.clear();
        self.has_data = false;
    }

    pub fn push(&mut self, chunk: &[u8]) -> Vec<Frame> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        // Lines are read in place and the consumed prefix is drained once at the end.
        let mut start = 0;
        loop {
            let from = start.max(self.scanned);
            let Some(rel) = self.buf[from..].iter().position(|&b| b == b'\n') else {
                break;
            };
            let pos = from + rel;
            let mut raw = &self.buf[start..pos];
            start = pos + 1;
            if self.skipping_line {
                self.skipping_line = false;
                continue;
            }
            if raw.last() == Some(&b'\r') {
                raw = &raw[..raw.len() - 1];
            }
            if raw.len() > MAX_LINE_BYTES {
                self.overflow(&mut out);
                continue;
            }
            let line = String::from_utf8_lossy(raw).into_owned();
            if line.is_empty() {
                if !self.drop_event && self.has_data {
                    out.push(Frame::Data(std::mem::take(&mut self.data)));
                }
                self.data.clear();
                self.has_data = false;
                self.drop_event = false;
                continue;
            }
            if line.starts_with(':') {
                continue;
            }
            let (field, value) = match line.find(':') {
                Some(i) => {
                    let raw = &line[i + 1..];
                    (&line[..i], raw.strip_prefix(' ').unwrap_or(raw))
                }
                None => (&line[..], ""),
            };
            if field == "data" && !self.drop_event {
                let sep = usize::from(self.has_data);
                if self.data.len() + sep + value.len() > MAX_EVENT_BYTES {
                    self.overflow(&mut out);
                } else {
                    if self.has_data {
                        self.data.push('\n');
                    }
                    self.data.push_str(value);
                    self.has_data = true;
                }
            }
        }
        self.buf.drain(..start);
        // What is left has no newline: skip it if mid-way through a dropped line, or cap it.
        if self.skipping_line {
            self.buf.clear();
        } else if self.buf.len() > MAX_LINE_BYTES {
            self.overflow(&mut out);
            self.skipping_line = true;
            self.buf.clear();
        }
        self.scanned = self.buf.len();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Frame {
        Frame::Data(s.into())
    }

    #[test]
    fn single_llama_swap_frame() {
        let mut p = SseParser::new();
        let out = p.push(b"event:message\ndata:{\"type\":\"x\"}\n\n");
        assert_eq!(out, [d(r#"{"type":"x"}"#)]);
    }

    #[test]
    fn two_frames_in_one_chunk() {
        let mut p = SseParser::new();
        let out = p.push(b"data:a\n\ndata:b\n\n");
        assert_eq!(out, [d("a"), d("b")]);
    }

    #[test]
    fn frame_split_across_chunks() {
        let mut p = SseParser::new();
        assert!(p.push(b"event:mess").is_empty());
        assert!(p.push(b"age\ndata:hel").is_empty());
        assert!(p.push(b"lo\n").is_empty());
        assert_eq!(p.push(b"\n"), [d("hello")]);
    }

    #[test]
    fn multibyte_utf8_split_across_chunks() {
        let bytes = "data:café\n\n".as_bytes();
        let split = bytes.iter().position(|&b| b == 0xC3).unwrap() + 1; // inside 'é'
        let mut p = SseParser::new();
        assert!(p.push(&bytes[..split]).is_empty());
        assert_eq!(p.push(&bytes[split..]), [d("café")]);
    }

    #[test]
    fn crlf_line_endings() {
        let mut p = SseParser::new();
        assert_eq!(p.push(b"data: x\r\n\r\n"), [d("x")]);
    }

    #[test]
    fn single_leading_space_is_stripped_only_once() {
        let mut p = SseParser::new();
        assert_eq!(p.push(b"data:  two\n\n"), [d(" two")]);
    }

    #[test]
    fn multi_line_data_is_joined_with_newline() {
        let mut p = SseParser::new();
        assert_eq!(p.push(b"data:line1\ndata:line2\n\n"), [d("line1\nline2")]);
    }

    #[test]
    fn comments_and_keepalives_are_ignored() {
        let mut p = SseParser::new();
        assert!(p.push(b": keep-alive\n\n").is_empty());
        assert_eq!(p.push(b":ping\ndata:x\n\n"), [d("x")]);
    }

    #[test]
    fn event_without_data_is_dropped() {
        let mut p = SseParser::new();
        assert!(p.push(b"event:message\nid:7\n\n").is_empty());
    }

    #[test]
    fn a_lone_empty_data_line_is_an_empty_event() {
        let mut p = SseParser::new();
        assert_eq!(p.push(b"data:\n\n"), [d("")]);
    }

    #[test]
    fn oversized_line_is_dropped_and_parsing_resumes() {
        let mut p = SseParser::new();
        let mut out = p.push(b"data:");
        let filler = vec![b'a'; 64 * 1024];
        let mut sent = 0;
        while sent < MAX_LINE_BYTES + 10 {
            out.extend(p.push(&filler));
            sent += filler.len();
        }
        out.extend(p.push(b"\n\ndata:ok\n\n"));
        assert_eq!(out, [Frame::Dropped, d("ok")]);
    }

    #[test]
    fn buffer_never_exceeds_line_cap() {
        let mut p = SseParser::new();
        let filler = vec![b'a'; 64 * 1024];
        let mut sent = 0;
        while sent < 3 * MAX_LINE_BYTES {
            p.push(&filler);
            sent += filler.len();
            assert!(p.buffered_len() <= MAX_LINE_BYTES);
        }
    }

    #[test]
    fn oversized_event_is_dropped() {
        let mut p = SseParser::new();
        let mut line = b"data:".to_vec();
        line.extend(vec![b'a'; 512 * 1024]);
        line.push(b'\n');
        let mut out = Vec::new();
        for _ in 0..(MAX_EVENT_BYTES / (512 * 1024) + 2) {
            out.extend(p.push(&line));
        }
        out.extend(p.push(b"\n"));
        out.extend(p.push(b"data:ok\n\n"));
        assert_eq!(out, [Frame::Dropped, d("ok")]);
    }

    #[test]
    fn empty_data_lines_count_toward_the_event_cap() {
        let mut p = SseParser::new();
        // Each empty `data:` line after the first adds one `\n` separator to the event.
        let lines = b"data:\n".repeat(64 * 1024);
        let mut out = Vec::new();
        for _ in 0..(MAX_EVENT_BYTES / (64 * 1024) + 1) {
            out.extend(p.push(&lines));
        }
        out.extend(p.push(b"\ndata:ok\n\n"));
        assert_eq!(out, [Frame::Dropped, d("ok")]);
    }

    #[test]
    fn each_dropped_event_is_reported_once() {
        let mut p = SseParser::new();
        let long_line = || {
            let mut l = b"data:".to_vec();
            l.extend(vec![b'a'; MAX_LINE_BYTES + 10]);
            l.push(b'\n');
            l
        };
        // Three dropped events, one frame each.
        for _ in 0..3 {
            let ev = [long_line(), b"\n".to_vec()].concat();
            assert_eq!(p.push(&ev), [Frame::Dropped]);
        }
        // One event with three oversized lines is still one dropped event.
        let mut ev = Vec::new();
        for _ in 0..3 {
            ev.extend(long_line());
        }
        ev.push(b'\n');
        assert_eq!(p.push(&ev), [Frame::Dropped]);
    }

    #[test]
    fn dropped_event_keeps_its_place_in_the_stream() {
        let mut p = SseParser::new();
        let mut input = b"data:before\n\ndata:".to_vec();
        input.extend(vec![b'a'; MAX_LINE_BYTES + 10]);
        input.extend_from_slice(b"\n\ndata:after\n\n");
        assert_eq!(p.push(&input), [d("before"), Frame::Dropped, d("after")]);
    }

    #[test]
    fn normal_frames_do_not_overflow() {
        let mut p = SseParser::new();
        let mut out = p.push(b"data:a\n\n");
        out.extend(p.push(b"data:b\n\n"));
        assert_eq!(out, [d("a"), d("b")]);
    }

    #[test]
    fn oversized_line_poisons_the_whole_event() {
        let mut p = SseParser::new();
        let mut input = b"data:x\ndata:".to_vec();
        input.extend(vec![b'a'; MAX_LINE_BYTES + 10]);
        input.extend_from_slice(b"\n\ndata:ok\n\n");
        assert_eq!(p.push(&input), [Frame::Dropped, d("ok")]);
    }
}
