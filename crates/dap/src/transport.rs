//! DAP 的消息分帧：和 LSP 一样，`Content-Length: N\r\n\r\n` 后跟 N 字节 JSON。

use std::io::{self, BufRead, Write};

use serde_json::Value;

/// 读一条消息。输入在消息边界处结束时返回 `Ok(None)`。
pub fn read_message(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut content_length: Option<usize> = None;
    let mut line = String::new();
    let mut saw_header = false;
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            if saw_header {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            return Ok(None);
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            if saw_header {
                break;
            }
            // 消息之间多余的空行
            continue;
        }
        saw_header = true;
        if let Some((name, value)) = trimmed.split_once(':') {
            if name.trim().eq_ignore_ascii_case("Content-Length") {
                let len = value.trim().parse::<usize>().map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "Content-Length 不是数字")
                })?;
                content_length = Some(len);
            }
        }
    }
    let len = content_length
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "缺少 Content-Length"))?;
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

pub fn write_message(writer: &mut impl Write, message: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(message)?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn round_trip() {
        let mut buf = Vec::new();
        let a = json!({"seq": 1, "type": "request", "command": "initialize"});
        let b = json!({"seq": 2, "type": "event", "event": "output", "body": {"output": "ä\n"}});
        write_message(&mut buf, &a).unwrap();
        write_message(&mut buf, &b).unwrap();
        let text = String::from_utf8(buf.clone()).unwrap();
        assert!(text.starts_with("Content-Length: "));
        let mut reader = io::BufReader::new(io::Cursor::new(buf));
        assert_eq!(read_message(&mut reader).unwrap(), Some(a));
        assert_eq!(read_message(&mut reader).unwrap(), Some(b));
        assert_eq!(read_message(&mut reader).unwrap(), None);
    }

    #[test]
    fn tolerant_headers() {
        let body = r#"{"seq":3}"#;
        let input = format!(
            "\r\ncontent-length:{}\nContent-Type: application/vscode-jsonrpc\n\n{body}",
            body.len()
        );
        let mut reader = io::BufReader::new(io::Cursor::new(input.into_bytes()));
        assert_eq!(read_message(&mut reader).unwrap(), Some(json!({"seq": 3})));
    }

    #[test]
    fn malformed_input() {
        let mut missing = io::BufReader::new(io::Cursor::new(b"Foo: 1\r\n\r\n{}".to_vec()));
        assert!(read_message(&mut missing).is_err());
        let mut truncated =
            io::BufReader::new(io::Cursor::new(b"Content-Length: 10\r\n\r\n{}".to_vec()));
        assert!(read_message(&mut truncated).is_err());
        let mut bad_json =
            io::BufReader::new(io::Cursor::new(b"Content-Length: 2\r\n\r\n{]".to_vec()));
        assert!(read_message(&mut bad_json).is_err());
    }
}
