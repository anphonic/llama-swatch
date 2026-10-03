//! Minimal Server-Sent Events parser.
//!
//! Feed raw bytes as they arrive from the network. Bytes are buffered until a
//! full line is available, so chunks may split lines or even UTF-8 code points.
//! Only the `data` field matters to us; `event`, `id`, `retry` and comments are ignored.

#[derive(Debug, Default)]
pub struct SseParser {
    buf: Vec<u8>,
    data: Vec<String>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let mut line: Vec<u8> = self.buf.drain(..=pos).collect();
            line.pop(); // '\n'
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let line = String::from_utf8_lossy(&line);
            if line.is_empty() {
                if !self.data.is_empty() {
                    out.push(self.data.join("\n"));
                    self.data.clear();
                }
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
            if field == "data" {
                self.data.push(value.to_string());
            }
        }
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
}
