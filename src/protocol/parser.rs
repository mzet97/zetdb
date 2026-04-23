use crate::domain::command::Command;
use bytes::Bytes;

#[derive(Debug, PartialEq)]
pub enum ParseError {
    EmptyCommand,
    UnknownCommand(String),
    SyntaxError(String),
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

    Err(ParseError::UnknownCommand(
        String::from_utf8_lossy(verb).into_owned(),
    ))
}

/// Legacy str-based parser — delegates to parse_bytes.
pub fn parse(input: &str) -> Result<Command, ParseError> {
    parse_bytes(input.as_bytes())
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
    // SAFETY: keys in our protocol are always valid UTF-8
    String::from_utf8_lossy(b).into_owned()
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
}
