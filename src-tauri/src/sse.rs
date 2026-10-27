//! Minimal Server-Sent Events parser.
//!
//! Feed raw bytes as they arrive from the network. Bytes are buffered until a
//! full line is available, so chunks may split lines or even UTF-8 code points.
//! Only the `data` field matters to us; `event`, `id`, `retry` and comments are ignored.

/// Longest single line we buffer; a server that never sends `\n` cannot grow memory past this.
/// The count includes a trailing `\r`.
pub const MAX_LINE_BYTES: usize = 1024 * 1024;
/// Most `data` bytes one event may carry before it is dropped.
pub const MAX_EVENT_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Default)]
pub struct SseParser {
    buf: Vec<u8>,
    /// Prefix of `buf` already known to hold no `\n`, so each byte is scanned once.
    scanned: usize,
    data: Vec<String>,
    data_bytes: usize,
    /// Discarding the rest of an over-long line up to its next `\n`.
    skipping_line: bool,
    /// The current event lost a line or grew too big; drop it at its blank line.
    drop_event: bool,
    overflowed: bool,
    /// Lines and events dropped for size since the last `take_overflows`.
    overflows: u32,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// True once any line or event has been dropped for size since this parser was created.
    pub fn overflowed(&self) -> bool {
        self.overflowed
    }

    /// Number of lines and events dropped for size since the last call; resets to 0.
    pub fn take_overflows(&mut self) -> u32 {
        std::mem::take(&mut self.overflows)
    }

    #[cfg(test)]
    fn buffered_len(&self) -> usize {
        self.buf.len()
    }

    fn overflow(&mut self) {
        self.overflowed = true;
        self.overflows = self.overflows.saturating_add(1);
        self.drop_event = true;
        self.data.clear();
        self.data_bytes = 0;
    }

    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
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
                self.overflow();
                continue;
            }
            let line = String::from_utf8_lossy(raw).into_owned();
            if line.is_empty() {
                if !self.drop_event && !self.data.is_empty() {
                    out.push(self.data.join("\n"));
                }
                self.data.clear();
                self.data_bytes = 0;
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
                self.data_bytes += value.len();
                if self.data_bytes > MAX_EVENT_BYTES {
                    self.overflow();
                } else {
                    self.data.push(value.to_string());
                }
            }
        }
        self.buf.drain(..start);
        // What is left has no newline: skip it if mid-way through a dropped line, or cap it.
        if self.skipping_line {
            self.buf.clear();
        } else if self.buf.len() > MAX_LINE_BYTES {
            self.overflow();
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

    #[test]
    fn single_llama_swap_frame() {
        let mut p = SseParser::new();
        let out = p.push(b"event:message\ndata:{\"type\":\"x\"}\n\n");
        assert_eq!(out, vec![r#"{"type":"x"}"#]);
    }

    #[test]
    fn two_frames_in_one_chunk() {
        let mut p = SseParser::new();
        let out = p.push(b"data:a\n\ndata:b\n\n");
        assert_eq!(out, vec!["a", "b"]);
    }

    #[test]
    fn frame_split_across_chunks() {
        let mut p = SseParser::new();
        assert!(p.push(b"event:mess").is_empty());
        assert!(p.push(b"age\ndata:hel").is_empty());
        assert!(p.push(b"lo\n").is_empty());
        assert_eq!(p.push(b"\n"), vec!["hello"]);
    }

    #[test]
    fn multibyte_utf8_split_across_chunks() {
        let bytes = "data:café\n\n".as_bytes();
        let split = bytes.iter().position(|&b| b == 0xC3).unwrap() + 1; // inside 'é'
        let mut p = SseParser::new();
        assert!(p.push(&bytes[..split]).is_empty());
        assert_eq!(p.push(&bytes[split..]), vec!["café"]);
    }

    #[test]
    fn crlf_line_endings() {
        let mut p = SseParser::new();
        assert_eq!(p.push(b"data: x\r\n\r\n"), vec!["x"]);
    }

    #[test]
    fn single_leading_space_is_stripped_only_once() {
        let mut p = SseParser::new();
        assert_eq!(p.push(b"data:  two\n\n"), vec![" two"]);
    }

    #[test]
    fn multi_line_data_is_joined_with_newline() {
        let mut p = SseParser::new();
        assert_eq!(p.push(b"data:line1\ndata:line2\n\n"), vec!["line1\nline2"]);
    }

    #[test]
    fn comments_and_keepalives_are_ignored() {
        let mut p = SseParser::new();
        assert!(p.push(b": keep-alive\n\n").is_empty());
        assert_eq!(p.push(b":ping\ndata:x\n\n"), vec!["x"]);
    }

    #[test]
    fn event_without_data_is_dropped() {
        let mut p = SseParser::new();
        assert!(p.push(b"event:message\nid:7\n\n").is_empty());
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
        assert_eq!(out, vec!["ok"]);
        assert!(p.overflowed());
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
        assert_eq!(out, vec!["ok"]);
        assert!(p.overflowed());
    }

    #[test]
    fn normal_frames_do_not_overflow() {
        let mut p = SseParser::new();
        let mut out = p.push(b"data:a\n\n");
        out.extend(p.push(b"data:b\n\n"));
        assert_eq!(out, vec!["a", "b"]);
        assert!(!p.overflowed());
    }

    #[test]
    fn each_dropped_event_is_counted_once() {
        let mut p = SseParser::new();
        let mut big = b"data:".to_vec();
        big.extend(vec![b'a'; MAX_LINE_BYTES + 10]);
        big.extend_from_slice(b"\n\n");
        for _ in 0..3 {
            assert!(p.push(&big).is_empty());
        }
        assert_eq!(p.take_overflows(), 3);
        assert_eq!(p.take_overflows(), 0);
        assert!(p.overflowed());
    }

    #[test]
    fn oversized_line_poisons_the_whole_event() {
        let mut p = SseParser::new();
        let mut input = b"data:x\ndata:".to_vec();
        input.extend(vec![b'a'; MAX_LINE_BYTES + 10]);
        input.extend_from_slice(b"\n\ndata:ok\n\n");
        assert_eq!(p.push(&input), vec!["ok"]);
    }
}
