use crate::domain::command::Command;
use bytes::Bytes;

#[derive(Debug, PartialEq)]
pub enum ParseError {
    EmptyCommand,
    UnknownCommand(String),
    SyntaxError(String),
}

#[derive(Debug)]
pub enum FrameResult {
    /// Complete command, advance buffer by `consumed` bytes
    Complete { consumed: usize, command: Command },
    /// Need more data from network
    Incomplete,
    /// Empty/whitespace line, advance buffer by `consumed` bytes
    Skip { consumed: usize },
}

/// Zero-alloc byte scanner parser. Operates on raw bytes without creating
/// Vec or calling to_uppercase. The only heap allocation is the key String
/// required by the Command enum.
pub fn parse_bytes(input: &[u8]) -> Result<Command, ParseError> {
    // Trim leading/trailing whitespace and \r\n
    let input = trim(input);
    if input.is_empty() {
        return Err(ParseError::EmptyCommand);
    }

    // Split into verb and rest at first whitespace
    let (verb, rest) = split_first_word(input);

    // Case-insensitive command match — no allocation
    if eq_ignore_ascii_case(verb, b"PING") {
        return Ok(Command::Ping);
    }
    if eq_ignore_ascii_case(verb, b"GET") {
        return parse_get_bytes(rest);
    }
    if eq_ignore_ascii_case(verb, b"SET") {
        return parse_set_bytes(rest);
    }
    if eq_ignore_ascii_case(verb, b"DEL") {
        return parse_del_bytes(rest);
    }
    if eq_ignore_ascii_case(verb, b"INCR") {
        return parse_incr_bytes(rest);
    }
    if eq_ignore_ascii_case(verb, b"INFO") {
        return Ok(Command::Info);
    }
    if eq_ignore_ascii_case(verb, b"DBSIZE") {
        return Ok(Command::DbSize);
    }

    Err(ParseError::UnknownCommand(
        String::from_utf8_lossy(verb).into_owned(),
    ))
}

/// Legacy str-based parser — delegates to parse_bytes.
pub fn parse(input: &str) -> Result<Command, ParseError> {
    parse_bytes(input.as_bytes())
}

/// Try to parse one complete frame from the buffer.
/// Auto-detects RESP (`*` prefix) vs inline protocol.
/// Returns `Ok(FrameResult::Complete)` for a parsed command,
/// `Ok(FrameResult::Skip)` for empty lines to skip,
/// `Ok(FrameResult::Incomplete)` when more data is needed.
pub fn try_parse_frame(buf: &[u8]) -> Result<FrameResult, ParseError> {
    if buf.is_empty() {
        return Ok(FrameResult::Incomplete);
    }

    if buf[0] == b'*' {
        parse_resp_frame(buf)
    } else {
        parse_inline_frame(buf)
    }
}

fn parse_get_bytes(rest: &[u8]) -> Result<Command, ParseError> {
    let rest = trim(rest);
    if rest.is_empty() {
        return Err(ParseError::SyntaxError(
            "GET requires exactly 1 argument: GET <key>".into(),
        ));
    }
    // Key is everything up to next whitespace
    let (key, trailing) = split_first_word(rest);
    let trailing = trim(trailing);
    if !trailing.is_empty() {
        return Err(ParseError::SyntaxError(
            "GET requires exactly 1 argument: GET <key>".into(),
        ));
    }
    Ok(Command::Get {
        key: bytes_to_string(key),
    })
}

fn parse_set_bytes(rest: &[u8]) -> Result<Command, ParseError> {
    let rest = trim(rest);
    if rest.is_empty() {
        return Err(ParseError::SyntaxError(
            "SET requires at least 2 arguments: SET <key> <value>".into(),
        ));
    }
    let (key, value_part) = split_first_word(rest);
    if key.is_empty() {
        return Err(ParseError::SyntaxError(
            "SET requires at least 2 arguments: SET <key> <value>".into(),
        ));
    }
    let value_part = trim(value_part);
    if value_part.is_empty() {
        return Err(ParseError::SyntaxError(
            "SET requires at least 2 arguments: SET <key> <value>".into(),
        ));
    }
    Ok(Command::Set {
        key: bytes_to_string(key),
        value: Bytes::copy_from_slice(value_part),
        ttl: None,
    })
}

fn parse_del_bytes(rest: &[u8]) -> Result<Command, ParseError> {
    let rest = trim(rest);
    if rest.is_empty() {
        return Err(ParseError::SyntaxError(
            "DEL requires exactly 1 argument: DEL <key>".into(),
        ));
    }
    let (key, trailing) = split_first_word(rest);
    let trailing = trim(trailing);
    if !trailing.is_empty() {
        return Err(ParseError::SyntaxError(
            "DEL requires exactly 1 argument: DEL <key>".into(),
        ));
    }
    Ok(Command::Del {
        key: bytes_to_string(key),
    })
}

fn parse_incr_bytes(rest: &[u8]) -> Result<Command, ParseError> {
    let rest = trim(rest);
    if rest.is_empty() {
        return Err(ParseError::SyntaxError(
            "INCR requires exactly 1 argument: INCR <key>".into(),
        ));
    }
    let (key, trailing) = split_first_word(rest);
    let trailing = trim(trailing);
    if !trailing.is_empty() {
        return Err(ParseError::SyntaxError(
            "INCR requires exactly 1 argument: INCR <key>".into(),
        ));
    }
    Ok(Command::Incr {
        key: bytes_to_string(key),
    })
}

// --- Frame parsers ---

fn parse_inline_frame(buf: &[u8]) -> Result<FrameResult, ParseError> {
    match memchr::memchr(b'\n', buf) {
        None => Ok(FrameResult::Incomplete),
        Some(pos) => match parse_bytes(&buf[..pos]) {
            Ok(cmd) => Ok(FrameResult::Complete {
                consumed: pos + 1,
                command: cmd,
            }),
            Err(ParseError::EmptyCommand) => Ok(FrameResult::Skip { consumed: pos + 1 }),
            Err(e) => Err(e),
        },
    }
}

fn parse_resp_frame(buf: &[u8]) -> Result<FrameResult, ParseError> {
    let mut pos = 0;

    // Array header: *<count>\r\n
    let (count, n) = match read_resp_line_int(buf, b'*')? {
        Some(v) => v,
        None => return Ok(FrameResult::Incomplete),
    };
    pos += n;

    if count < 0 {
        return Err(ParseError::SyntaxError("RESP: null array".into()));
    }
    let count = count as usize;
    if count > 1024 {
        return Err(ParseError::SyntaxError("RESP: array too large".into()));
    }
    if count == 0 {
        return Err(ParseError::EmptyCommand);
    }

    // Parse first bulk string (verb) — zero alloc
    let (verb, n) = match read_resp_bulk(&buf[pos..])? {
        Some(v) => v,
        None => return Ok(FrameResult::Incomplete),
    };
    pos += n;
    let args_remaining = count - 1;

    // Switch on verb and parse args inline — no Vec allocation
    if eq_ignore_ascii_case(verb, b"PING") {
        return Ok(FrameResult::Complete { consumed: pos, command: Command::Ping });
    }
    if eq_ignore_ascii_case(verb, b"INFO") {
        return Ok(FrameResult::Complete { consumed: pos, command: Command::Info });
    }
    if eq_ignore_ascii_case(verb, b"DBSIZE") {
        return Ok(FrameResult::Complete { consumed: pos, command: Command::DbSize });
    }
    if eq_ignore_ascii_case(verb, b"GET") {
        if args_remaining != 1 {
            return Err(ParseError::SyntaxError(
                "GET requires exactly 1 argument: GET <key>".into(),
            ));
        }
        let (key_data, n) = match read_resp_bulk(&buf[pos..])? {
            Some(v) => v,
            None => return Ok(FrameResult::Incomplete),
        };
        pos += n;
        return Ok(FrameResult::Complete {
            consumed: pos,
            command: Command::Get {
                key: bytes_to_string(key_data),
            },
        });
    }
    if eq_ignore_ascii_case(verb, b"SET") {
        if args_remaining < 2 {
            return Err(ParseError::SyntaxError(
                "SET requires at least 2 arguments: SET <key> <value>".into(),
            ));
        }
        let (key_data, n) = match read_resp_bulk(&buf[pos..])? {
            Some(v) => v,
            None => return Ok(FrameResult::Incomplete),
        };
        pos += n;
        let (val_data, n) = match read_resp_bulk(&buf[pos..])? {
            Some(v) => v,
            None => return Ok(FrameResult::Incomplete),
        };
        pos += n;
        // Skip any extra args (future: PX support)
        for _ in 2..args_remaining {
            let (_, n) = match read_resp_bulk(&buf[pos..])? {
                Some(v) => v,
                None => return Ok(FrameResult::Incomplete),
            };
            pos += n;
        }
        return Ok(FrameResult::Complete {
            consumed: pos,
            command: Command::Set {
                key: bytes_to_string(key_data),
                value: Bytes::copy_from_slice(val_data),
                ttl: None,
            },
        });
    }
    if eq_ignore_ascii_case(verb, b"DEL") {
        if args_remaining != 1 {
            return Err(ParseError::SyntaxError(
                "DEL requires exactly 1 argument: DEL <key>".into(),
            ));
        }
        let (key_data, n) = match read_resp_bulk(&buf[pos..])? {
            Some(v) => v,
            None => return Ok(FrameResult::Incomplete),
        };
        pos += n;
        return Ok(FrameResult::Complete {
            consumed: pos,
            command: Command::Del {
                key: bytes_to_string(key_data),
            },
        });
    }
    if eq_ignore_ascii_case(verb, b"INCR") {
        if args_remaining != 1 {
            return Err(ParseError::SyntaxError(
                "INCR requires exactly 1 argument: INCR <key>".into(),
            ));
        }
        let (key_data, n) = match read_resp_bulk(&buf[pos..])? {
            Some(v) => v,
            None => return Ok(FrameResult::Incomplete),
        };
        pos += n;
        return Ok(FrameResult::Complete {
            consumed: pos,
            command: Command::Incr {
                key: bytes_to_string(key_data),
            },
        });
    }

    Err(ParseError::UnknownCommand(bytes_to_string(verb)))
}

/// Parse a RESP integer line like `*3\r\n` or `$5\r\n`.
/// Returns `(value, bytes_consumed)` or `None` if incomplete.
fn read_resp_line_int(buf: &[u8], prefix: u8) -> Result<Option<(i64, usize)>, ParseError> {
    if buf.is_empty() {
        return Ok(None);
    }
    if buf[0] != prefix {
        let ch = buf[0] as char;
        let expected = prefix as char;
        return Err(ParseError::SyntaxError(format!(
            "RESP: expected '{expected}', got '{ch}'"
        )));
    }

    match memchr::memchr(b'\n', buf) {
        None => Ok(None),
        Some(pos) => {
            let num_end = if pos > 0 && buf[pos - 1] == b'\r' {
                pos - 1
            } else {
                pos
            };
            let num_str = &buf[1..num_end];
            let s = std::str::from_utf8(num_str)
                .map_err(|_| ParseError::SyntaxError("RESP: invalid integer".into()))?;
            let n: i64 = s
                .parse()
                .map_err(|_| ParseError::SyntaxError("RESP: expected integer".into()))?;
            Ok(Some((n, pos + 1)))
        }
    }
}

/// Parse a single RESP bulk string: `$<len>\r\n<data>\r\n`.
/// Returns `(data_slice, bytes_consumed)` or `None` if incomplete.
fn read_resp_bulk(buf: &[u8]) -> Result<Option<(&[u8], usize)>, ParseError> {
    let (len, n) = match read_resp_line_int(buf, b'$')? {
        Some(v) => v,
        None => return Ok(None),
    };
    let mut pos = n;

    if len < 0 {
        // Null bulk string
        return Ok(Some((&buf[..0], pos)));
    }
    let len = len as usize;
    if pos + len + 2 > buf.len() {
        return Ok(None);
    }
    let data = &buf[pos..pos + len];
    pos += len + 2;
    Ok(Some((data, pos)))
}

// --- Byte helpers ---

fn trim(buf: &[u8]) -> &[u8] {
    let mut start = 0;
    let mut end = buf.len();
    while start < end && is_whitespace(buf[start]) {
        start += 1;
    }
    while end > start && is_whitespace(buf[end - 1]) {
        end -= 1;
    }
    &buf[start..end]
}

fn split_first_word(buf: &[u8]) -> (&[u8], &[u8]) {
    match buf.iter().position(|&b| is_whitespace(b)) {
        Some(i) => (&buf[..i], &buf[i..]),
        None => (buf, &buf[buf.len()..]),
    }
}

fn is_whitespace(b: u8) -> bool {
    b == b' ' || b == b'\t' || b == b'\r' || b == b'\n'
}

fn eq_ignore_ascii_case(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.eq_ignore_ascii_case(y))
}

fn bytes_to_string(b: &[u8]) -> String {
    // SAFETY: keys in our protocol are always valid UTF-8 (delimited by \r\n, no null bytes)
    unsafe { String::from_utf8_unchecked(b.to_vec()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::command::Command;

    #[test]
    fn ping_case_insensitive() {
        assert!(matches!(parse("PING"), Ok(Command::Ping)));
        assert!(matches!(parse("ping"), Ok(Command::Ping)));
        assert!(matches!(parse("Ping"), Ok(Command::Ping)));
    }

    #[test]
    fn get_valid() {
        let cmd = parse("GET mykey").unwrap();
        assert!(matches!(cmd, Command::Get { ref key } if key == "mykey"));
    }

    #[test]
    fn set_valid() {
        match parse("SET mykey hello").unwrap() {
            Command::Set { key, value, ttl } => {
                assert_eq!(key, "mykey");
                assert_eq!(value, Bytes::from("hello"));
                assert!(ttl.is_none());
            }
            other => panic!("Expected Set, got {other:?}"),
        }
    }

    #[test]
    fn set_multi_word_value() {
        match parse("SET msg hello world foo").unwrap() {
            Command::Set { key, value, .. } => {
                assert_eq!(key, "msg");
                assert_eq!(value, Bytes::from("hello world foo"));
            }
            other => panic!("Expected Set, got {other:?}"),
        }
    }

    #[test]
    fn del_valid() {
        let cmd = parse("DEL mykey").unwrap();
        assert!(matches!(cmd, Command::Del { ref key } if key == "mykey"));
    }

    #[test]
    fn incr_valid() {
        let cmd = parse("INCR counter").unwrap();
        assert!(matches!(cmd, Command::Incr { ref key } if key == "counter"));
    }

    #[test]
    fn empty_input() {
        assert_eq!(parse(""), Err(ParseError::EmptyCommand));
        assert_eq!(parse("   "), Err(ParseError::EmptyCommand));
        assert_eq!(parse("\r\n"), Err(ParseError::EmptyCommand));
    }

    #[test]
    fn unknown_command() {
        assert!(matches!(parse("FOOBAR"), Err(ParseError::UnknownCommand(_))));
    }

    #[test]
    fn get_missing_key() {
        assert!(matches!(parse("GET"), Err(ParseError::SyntaxError(_))));
    }

    #[test]
    fn get_extra_args() {
        assert!(matches!(parse("GET a b"), Err(ParseError::SyntaxError(_))));
    }

    #[test]
    fn set_missing_args() {
        assert!(matches!(parse("SET"), Err(ParseError::SyntaxError(_))));
        assert!(matches!(parse("SET key"), Err(ParseError::SyntaxError(_))));
    }

    #[test]
    fn del_missing_key() {
        assert!(matches!(parse("DEL"), Err(ParseError::SyntaxError(_))));
    }

    #[test]
    fn incr_missing_key() {
        assert!(matches!(parse("INCR"), Err(ParseError::SyntaxError(_))));
    }

    #[test]
    fn extra_whitespace() {
        let cmd = parse("  GET   mykey  ").unwrap();
        assert!(matches!(cmd, Command::Get { ref key } if key == "mykey"));
    }

    #[test]
    fn crlf_terminator() {
        let cmd = parse("GET mykey\r\n").unwrap();
        assert!(matches!(cmd, Command::Get { ref key } if key == "mykey"));
    }

    // --- parse_bytes specific tests ---

    #[test]
    fn parse_bytes_get() {
        let cmd = parse_bytes(b"GET mykey").unwrap();
        assert!(matches!(cmd, Command::Get { ref key } if key == "mykey"));
    }

    #[test]
    fn parse_bytes_set() {
        let cmd = parse_bytes(b"SET k v\r\n").unwrap();
        match cmd {
            Command::Set { key, value, .. } => {
                assert_eq!(key, "k");
                assert_eq!(value, Bytes::from("v"));
            }
            other => panic!("Expected Set, got {other:?}"),
        }
    }

    #[test]
    fn parse_bytes_empty() {
        assert_eq!(parse_bytes(b""), Err(ParseError::EmptyCommand));
        assert_eq!(parse_bytes(b"   \r\n"), Err(ParseError::EmptyCommand));
    }

    #[test]
    fn parse_bytes_unknown() {
        assert!(matches!(parse_bytes(b"FOO"), Err(ParseError::UnknownCommand(_))));
    }

    // --- Frame parser tests ---

    #[test]
    fn frame_inline_ping() {
        let input = b"PING\r\n";
        match try_parse_frame(input).unwrap() {
            FrameResult::Complete { consumed, command } => {
                assert_eq!(consumed, 6);
                assert!(matches!(command, Command::Ping));
            }
            other => panic!("Expected Complete, got {other:?}"),
        }
    }

    #[test]
    fn frame_inline_incomplete() {
        let input = b"PING";
        match try_parse_frame(input).unwrap() {
            FrameResult::Incomplete => {}
            other => panic!("Expected Incomplete, got {other:?}"),
        }
    }

    #[test]
    fn frame_inline_empty_line() {
        let input = b"\r\n";
        match try_parse_frame(input).unwrap() {
            FrameResult::Skip { consumed } => assert_eq!(consumed, 2),
            other => panic!("Expected Skip, got {other:?}"),
        }
    }

    #[test]
    fn frame_inline_get() {
        let input = b"GET mykey\r\n";
        match try_parse_frame(input).unwrap() {
            FrameResult::Complete { consumed, command } => {
                assert_eq!(consumed, 11);
                assert!(matches!(command, Command::Get { ref key } if key == "mykey"));
            }
            other => panic!("Expected Complete, got {other:?}"),
        }
    }

    #[test]
    fn resp_ping() {
        let input = b"*1\r\n$4\r\nPING\r\n";
        match try_parse_frame(input).unwrap() {
            FrameResult::Complete { consumed, command } => {
                assert_eq!(consumed, 14);
                assert!(matches!(command, Command::Ping));
            }
            other => panic!("Expected Complete, got {other:?}"),
        }
    }

    #[test]
    fn resp_get() {
        let input = b"*2\r\n$3\r\nGET\r\n$5\r\nmykey\r\n";
        match try_parse_frame(input).unwrap() {
            FrameResult::Complete { consumed, command } => {
                assert_eq!(consumed, 24);
                assert!(matches!(command, Command::Get { ref key } if key == "mykey"));
            }
            other => panic!("Expected Complete, got {other:?}"),
        }
    }

    #[test]
    fn resp_set() {
        let input = b"*3\r\n$3\r\nSET\r\n$5\r\nmykey\r\n$5\r\nhello\r\n";
        match try_parse_frame(input).unwrap() {
            FrameResult::Complete { command, .. } => match command {
                Command::Set { key, value, .. } => {
                    assert_eq!(key, "mykey");
                    assert_eq!(value, Bytes::from("hello"));
                }
                other => panic!("Expected Set, got {other:?}"),
            },
            other => panic!("Expected Complete, got {other:?}"),
        }
    }

    #[test]
    fn resp_del() {
        let input = b"*2\r\n$3\r\nDEL\r\n$5\r\nmykey\r\n";
        match try_parse_frame(input).unwrap() {
            FrameResult::Complete { command, .. } => {
                assert!(matches!(command, Command::Del { ref key } if key == "mykey"));
            }
            other => panic!("Expected Complete, got {other:?}"),
        }
    }

    #[test]
    fn resp_incr() {
        let input = b"*2\r\n$4\r\nINCR\r\n$3\r\ncnt\r\n";
        match try_parse_frame(input).unwrap() {
            FrameResult::Complete { command, .. } => {
                assert!(matches!(command, Command::Incr { ref key } if key == "cnt"));
            }
            other => panic!("Expected Complete, got {other:?}"),
        }
    }

    #[test]
    fn resp_incomplete_header() {
        let input = b"*2\r\n";
        match try_parse_frame(input).unwrap() {
            FrameResult::Incomplete => {}
            other => panic!("Expected Incomplete, got {other:?}"),
        }
    }

    #[test]
    fn resp_incomplete_bulk() {
        let input = b"*2\r\n$3\r\nGET\r\n$5\r\nmyk";
        match try_parse_frame(input).unwrap() {
            FrameResult::Incomplete => {}
            other => panic!("Expected Incomplete, got {other:?}"),
        }
    }

    #[test]
    fn resp_empty_array() {
        let input = b"*0\r\n";
        assert!(matches!(try_parse_frame(input), Err(ParseError::EmptyCommand)));
    }

    #[test]
    fn resp_null_array() {
        let input = b"*-1\r\n";
        assert!(matches!(try_parse_frame(input), Err(ParseError::SyntaxError(_))));
    }

    #[test]
    fn resp_case_insensitive() {
        let input = b"*2\r\n$3\r\nget\r\n$1\r\nx\r\n";
        match try_parse_frame(input).unwrap() {
            FrameResult::Complete { command, .. } => {
                assert!(matches!(command, Command::Get { ref key } if key == "x"));
            }
            other => panic!("Expected Complete, got {other:?}"),
        }
    }

    #[test]
    fn resp_two_frames_in_buffer() {
        let input = b"*1\r\n$4\r\nPING\r\n*2\r\n$3\r\nGET\r\n$1\r\nx\r\n";
        match try_parse_frame(input).unwrap() {
            FrameResult::Complete { consumed, command } => {
                assert!(matches!(command, Command::Ping));
                assert_eq!(consumed, 14);
                // Second frame
                match try_parse_frame(&input[consumed..]).unwrap() {
                    FrameResult::Complete { command, .. } => {
                        assert!(matches!(command, Command::Get { ref key } if key == "x"));
                    }
                    other => panic!("Expected Complete for second frame, got {other:?}"),
                }
            }
            other => panic!("Expected Complete, got {other:?}"),
        }
    }

    #[test]
    fn resp_null_bulk_string_in_array() {
        // SET with null value — becomes empty bytes
        let input = b"*3\r\n$3\r\nSET\r\n$1\r\nk\r\n$-1\r\n";
        match try_parse_frame(input).unwrap() {
            FrameResult::Complete { command, .. } => match command {
                Command::Set { key, value, .. } => {
                    assert_eq!(key, "k");
                    assert_eq!(value, Bytes::from(""));
                }
                other => panic!("Expected Set, got {other:?}"),
            },
            other => panic!("Expected Complete, got {other:?}"),
        }
    }

    #[test]
    fn resp_unknown_command() {
        let input = b"*1\r\n$3\r\nFOO\r\n";
        assert!(matches!(try_parse_frame(input), Err(ParseError::UnknownCommand(_))));
    }

    #[test]
    fn resp_wrong_arg_count() {
        let input = b"*1\r\n$3\r\nGET\r\n";
        assert!(matches!(try_parse_frame(input), Err(ParseError::SyntaxError(_))));
    }
}
