// Otimizado para ParsedCommand - zero alocação no hot path
use std::time::Duration;
use crate::domain::parsed_command::ParsedCommand;
use crate::protocol::parser::{ParseError, trim, split_first_word, eq_ignore_ascii_case};

/// Find the end of the first command (first \r\n after the command, including any trailing whitespace).
#[allow(dead_code)]
#[inline(always)]
fn find_command_end(buf: &[u8]) -> usize {
    // Find first \r\n
    for i in 0..buf.len().saturating_sub(1) {
        if buf[i] == b'\r' && buf[i + 1] == b'\n' {
            return i + 2; // Include \r\n
        }
    }
    // No \r\n found — return full buffer (should not happen for complete commands)
    buf.len()
}

/// Parse inline command into borrowed ParsedCommand (zero allocation).
/// Returns FrameResult with borrowed command or error.
/// Only parses the first complete command (up to first \r\n), leaving the rest for subsequent calls.
pub fn parse_inline_borrowed<'a>(original_input: &'a [u8]) -> Result<FrameResultBorrowed<'a>, ParseError> {
    // Use memchr for fast \r\n scanning (SIMD-accelerated)
    let pos = match memchr::memchr2(b'\r', b'\n', original_input) {
        Some(p) if p + 1 < original_input.len() && original_input[p] == b'\r' && original_input[p + 1] == b'\n' => p,
        Some(p) if original_input[p] == b'\n' => p.saturating_sub(1), // Handle bare \n
        _ => {
            // No complete command yet - need more data
            return Ok(FrameResultBorrowed::Incomplete);
        }
    };
    
    // pos now points to \r, command ends at pos + 2 (including \r\n)
    let command_end = pos + 2;
    let input = trim(&original_input[..pos]);
    
    if input.is_empty() {
        // Empty line (just \r\n), skip it
        return Ok(FrameResultBorrowed::Skip { consumed: command_end });
    }

    let (verb, rest) = split_first_word(input);

    // Reordenado por frequência: GET/SET primeiro
    if eq_ignore_ascii_case(verb, b"GET") {
        return parse_get_borrowed(original_input, rest, command_end);
    }
    if eq_ignore_ascii_case(verb, b"SET") {
        return parse_set_borrowed(original_input, rest, command_end);
    }
    if eq_ignore_ascii_case(verb, b"PING") {
        return Ok(FrameResultBorrowed::Complete { 
            consumed: command_end, 
            command: ParsedCommand::Ping 
        });
    }
    if eq_ignore_ascii_case(verb, b"DEL") {
        return parse_del_borrowed(original_input, rest, command_end);
    }
    if eq_ignore_ascii_case(verb, b"INCR") {
        return parse_incr_borrowed(original_input, rest, command_end);
    }
    if eq_ignore_ascii_case(verb, b"INFO") {
        return Ok(FrameResultBorrowed::Complete { 
            consumed: command_end, 
            command: ParsedCommand::Info 
        });
    }
    if eq_ignore_ascii_case(verb, b"DBSIZE") {
        return Ok(FrameResultBorrowed::Complete { 
            consumed: command_end, 
            command: ParsedCommand::DbSize 
        });
    }
    if eq_ignore_ascii_case(verb, b"EXISTS") {
        return parse_single_key_borrowed(original_input, rest, command_end, |key| ParsedCommand::Exists { key });
    }
    if eq_ignore_ascii_case(verb, b"TTL") {
        return parse_single_key_borrowed(original_input, rest, command_end, |key| ParsedCommand::Ttl { key });
    }
    if eq_ignore_ascii_case(verb, b"EXPIRE") {
        return parse_expire_borrowed(original_input, rest, command_end);
    }
    if eq_ignore_ascii_case(verb, b"FLUSHDB") {
        let rest = trim(rest);
        if !rest.is_empty() {
            return Err(ParseError::SyntaxError("FLUSHDB takes no arguments".into()));
        }
        return Ok(FrameResultBorrowed::Complete { 
            consumed: command_end, 
            command: ParsedCommand::FlushDb 
        });
    }
    if eq_ignore_ascii_case(verb, b"KEYS") {
        let rest = trim(rest);
        if !rest.is_empty() {
            return Err(ParseError::SyntaxError("KEYS takes no arguments".into()));
        }
        return Ok(FrameResultBorrowed::Complete { 
            consumed: command_end, 
            command: ParsedCommand::Keys 
        });
    }
    if eq_ignore_ascii_case(verb, b"MGET") {
        return parse_mget_borrowed(original_input, rest, command_end);
    }
    if eq_ignore_ascii_case(verb, b"MSET") {
        return parse_mset_borrowed(original_input, rest, command_end);
    }

    Err(ParseError::UnknownCommand(
        String::from_utf8_lossy(verb).into_owned(),
    ))
}

#[derive(Debug)]
pub enum FrameResultBorrowed<'a> {
    Complete { consumed: usize, command: ParsedCommand<'a> },
    Incomplete,
    Skip { consumed: usize },
}

fn parse_get_borrowed<'a>(_original_input: &'a [u8], rest: &'a [u8], command_end: usize) -> Result<FrameResultBorrowed<'a>, ParseError> {
    let rest_trimmed = trim(rest);
    if rest_trimmed.is_empty() {
        return Err(ParseError::SyntaxError(
            "GET requires exactly 1 argument: GET <key>".into(),
        ));
    }
    let (key, trailing) = split_first_word(rest_trimmed);
    let trailing = trim(trailing);
    if !trailing.is_empty() {
        return Err(ParseError::SyntaxError(
            "GET requires exactly 1 argument: GET <key>".into(),
        ));
    }
    Ok(FrameResultBorrowed::Complete {
        consumed: command_end,
        command: ParsedCommand::Get { key },
    })
}

fn parse_set_borrowed<'a>(_original_input: &'a [u8], rest: &'a [u8], command_end: usize) -> Result<FrameResultBorrowed<'a>, ParseError> {
    let rest_trimmed = trim(rest);
    if rest_trimmed.is_empty() {
        return Err(ParseError::SyntaxError(
            "SET requires at least 2 arguments: SET <key> <value>".into(),
        ));
    }
    let (key, value_part) = split_first_word(rest_trimmed);
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
    // In pipelined mode, value_part may contain "\r\nSET ..." from subsequent commands.
    // We must only take the first token as the value, just like the original parser.
    let (value, trailing) = split_first_word(value_part);
    let trailing = trim(trailing);
    // Check for TTL (optional 4th argument)
    let ttl = if !trailing.is_empty() {
        let (ttl_str, after_ttl) = split_first_word(trailing);
        let after_ttl = trim(after_ttl);
        if !after_ttl.is_empty() {
            return Err(ParseError::SyntaxError(
                "SET requires at most 4 arguments: SET <key> <value> [EX <seconds>]".into(),
            ));
        }
        parse_u64_ascii(ttl_str).map(Duration::from_secs)
    } else {
        None
    };
    Ok(FrameResultBorrowed::Complete {
        consumed: command_end,
        command: ParsedCommand::Set {
            key,
            value,
            ttl,
        },
    })
}

fn parse_del_borrowed<'a>(_original_input: &'a [u8], rest: &'a [u8], command_end: usize) -> Result<FrameResultBorrowed<'a>, ParseError> {
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
    Ok(FrameResultBorrowed::Complete {
        consumed: command_end,
        command: ParsedCommand::Del { key },
    })
}

fn parse_incr_borrowed<'a>(_original_input: &'a [u8], rest: &'a [u8], command_end: usize) -> Result<FrameResultBorrowed<'a>, ParseError> {
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
    Ok(FrameResultBorrowed::Complete {
        consumed: command_end,
        command: ParsedCommand::Incr { key },
    })
}

fn parse_single_key_borrowed<'a>(
    _original_input: &'a [u8],
    rest: &'a [u8],
    command_end: usize,
    constructor: fn(&'a [u8]) -> ParsedCommand<'a>,
) -> Result<FrameResultBorrowed<'a>, ParseError> {
    let rest = trim(rest);
    if rest.is_empty() {
        return Err(ParseError::SyntaxError(
            "Command requires exactly 1 argument".into(),
        ));
    }
    let (key, trailing) = split_first_word(rest);
    let trailing = trim(trailing);
    if !trailing.is_empty() {
        return Err(ParseError::SyntaxError(
            "Command requires exactly 1 argument".into(),
        ));
    }
    Ok(FrameResultBorrowed::Complete {
        consumed: command_end,
        command: constructor(key),
    })
}

fn parse_expire_borrowed<'a>(_original_input: &'a [u8], rest: &'a [u8], command_end: usize) -> Result<FrameResultBorrowed<'a>, ParseError> {
    let rest = trim(rest);
    if rest.is_empty() {
        return Err(ParseError::SyntaxError(
            "EXPIRE requires exactly 2 arguments: EXPIRE <key> <seconds>".into(),
        ));
    }
    let (key, seconds_part) = split_first_word(rest);
    let seconds_part = trim(seconds_part);
    if seconds_part.is_empty() {
        return Err(ParseError::SyntaxError(
            "EXPIRE requires exactly 2 arguments: EXPIRE <key> <seconds>".into(),
        ));
    }
    let seconds = parse_u64_ascii(seconds_part)
        .ok_or_else(|| ParseError::SyntaxError("EXPIRE: seconds must be a positive integer".into()))?;
    Ok(FrameResultBorrowed::Complete {
        consumed: command_end,
        command: ParsedCommand::Expire { key, seconds },
    })
}

fn parse_mget_borrowed<'a>(_original_input: &'a [u8], rest: &'a [u8], command_end: usize) -> Result<FrameResultBorrowed<'a>, ParseError> {
    let rest = trim(rest);
    if rest.is_empty() {
        return Err(ParseError::SyntaxError(
            "MGET requires at least 1 key".into(),
        ));
    }
    // Collect all keys
    let mut keys = Vec::new();
    let mut remaining = rest;
    while !remaining.is_empty() {
        let (key, trailing) = split_first_word(remaining);
        if !key.is_empty() {
            keys.push(key);
        }
        remaining = trim(trailing);
    }
    if keys.is_empty() {
        return Err(ParseError::SyntaxError(
            "MGET requires at least 1 key".into(),
        ));
    }
    Ok(FrameResultBorrowed::Complete {
        consumed: command_end,
        command: ParsedCommand::Mget { keys },
    })
}

fn parse_mset_borrowed<'a>(_original_input: &'a [u8], rest: &'a [u8], command_end: usize) -> Result<FrameResultBorrowed<'a>, ParseError> {
    let rest = trim(rest);
    if rest.is_empty() {
        return Err(ParseError::SyntaxError(
            "MSET requires at least 2 arguments: MSET <key> <value>".into(),
        ));
    }
    // Collect all key-value pairs
    let mut pairs = Vec::new();
    let mut remaining = rest;
    while !remaining.is_empty() {
        let (key, trailing) = split_first_word(remaining);
        if key.is_empty() {
            break;
        }
        remaining = trim(trailing);
        if remaining.is_empty() {
            return Err(ParseError::SyntaxError(
                "MSET requires even number of arguments".into(),
            ));
        }
        let (value, trailing) = split_first_word(remaining);
        pairs.push((key, value));
        remaining = trim(trailing);
    }
    if pairs.is_empty() {
        return Err(ParseError::SyntaxError(
            "MSET requires at least 2 arguments: MSET <key> <value>".into(),
        ));
    }
    Ok(FrameResultBorrowed::Complete {
        consumed: command_end,
        command: ParsedCommand::Mset { pairs },
    })
}

#[inline(always)]
fn parse_u64_ascii(buf: &[u8]) -> Option<u64> {
    if buf.is_empty() {
        return None;
    }
    let mut n: u64 = 0;
    for &b in buf {
        if !b.is_ascii_digit() {
            return None;
        }
        n = n * 10 + u64::from(b - b'0');
    }
    Some(n)
}
