use bytes::{Bytes, BytesMut};

#[derive(Debug, PartialEq)]
pub enum ResponseError {
    UnknownCommand(String),
    SyntaxError(String),
    TypeError(String),
    NotFound(String),
    InternalError(String),
}

#[derive(Debug, PartialEq)]
pub enum Response {
    Pong,
    Ok,
    Value(Option<Bytes>),
    Integer(i64),
    Error(ResponseError),
    Array(Vec<Option<Bytes>>),
}

impl Response {
    /// Returns true if the response is not an error.
    pub fn is_success(&self) -> bool {
        !matches!(self, Response::Error(_))
    }

    /// Writes serialized response in inline format (simple strings for values).
    pub fn write_to(&self, buf: &mut BytesMut) {
        self.write_to_impl(buf, false);
    }

    /// Writes serialized response in RESP format (bulk strings for values).
    pub fn write_to_resp(&self, buf: &mut BytesMut) {
        self.write_to_impl(buf, true);
    }

    fn write_to_impl(&self, buf: &mut BytesMut, resp_mode: bool) {
        match self {
            Self::Pong => buf.extend_from_slice(b"+PONG\r\n"),
            Self::Ok => buf.extend_from_slice(b"+OK\r\n"),
            Self::Value(Some(data)) => {
                if resp_mode {
                    // RESP bulk string: $<len>\r\n<data>\r\n
                    buf.extend_from_slice(b"$");
                    let mut itoa_buf = itoa::Buffer::new();
                    buf.extend_from_slice(itoa_buf.format(data.len()).as_bytes());
                    buf.extend_from_slice(b"\r\n");
                    buf.extend_from_slice(data);
                    buf.extend_from_slice(b"\r\n");
                } else {
                    // Inline simple string: +<data>\r\n
                    buf.extend_from_slice(b"+");
                    buf.extend_from_slice(data);
                    buf.extend_from_slice(b"\r\n");
                }
            }
            Self::Value(None) => buf.extend_from_slice(b"$-1\r\n"),
            Self::Integer(n) => {
                buf.extend_from_slice(b":");
                let mut itoa_buf = itoa::Buffer::new();
                buf.extend_from_slice(itoa_buf.format(*n).as_bytes());
                buf.extend_from_slice(b"\r\n");
            }
            Self::Error(ResponseError::UnknownCommand(cmd)) => {
                buf.extend_from_slice(b"-ERR unknown command '");
                buf.extend_from_slice(cmd.as_bytes());
                buf.extend_from_slice(b"'\r\n");
            }
            Self::Error(ResponseError::SyntaxError(msg)) => {
                buf.extend_from_slice(b"-ERR syntax: ");
                buf.extend_from_slice(msg.as_bytes());
                buf.extend_from_slice(b"\r\n");
            }
            Self::Error(ResponseError::TypeError(msg)) => {
                buf.extend_from_slice(b"-ERR type: ");
                buf.extend_from_slice(msg.as_bytes());
                buf.extend_from_slice(b"\r\n");
            }
            Self::Error(ResponseError::NotFound(key)) => {
                buf.extend_from_slice(b"-ERR not found: ");
                buf.extend_from_slice(key.as_bytes());
                buf.extend_from_slice(b"\r\n");
            }
            Self::Error(ResponseError::InternalError(msg)) => {
                buf.extend_from_slice(b"-ERR internal: ");
                buf.extend_from_slice(msg.as_bytes());
                buf.extend_from_slice(b"\r\n");
            }
            Self::Array(items) => {
                let mut itoa_buf = itoa::Buffer::new();
                buf.extend_from_slice(b"*");
                buf.extend_from_slice(itoa_buf.format(items.len()).as_bytes());
                buf.extend_from_slice(b"\r\n");
                for item in items {
                    match item {
                        Some(data) => {
                            buf.extend_from_slice(b"$");
                            buf.extend_from_slice(itoa_buf.format(data.len()).as_bytes());
                            buf.extend_from_slice(b"\r\n");
                            buf.extend_from_slice(data);
                            buf.extend_from_slice(b"\r\n");
                        }
                        None => {
                            buf.extend_from_slice(b"$-1\r\n");
                        }
                    }
                }
            }
        }
    }

    pub fn serialize(&self) -> String {
        match self {
            Self::Pong => "+PONG\r\n".into(),
            Self::Ok => "+OK\r\n".into(),
            Self::Value(Some(data)) => {
                let val = String::from_utf8_lossy(data);
                format!("+{val}\r\n")
            }
            Self::Value(None) => "$-1\r\n".into(),
            Self::Integer(n) => format!(":{n}\r\n"),
            Self::Error(ResponseError::UnknownCommand(cmd)) => {
                format!("-ERR unknown command '{cmd}'\r\n")
            }
            Self::Error(ResponseError::SyntaxError(msg)) => {
                format!("-ERR syntax: {msg}\r\n")
            }
            Self::Error(ResponseError::TypeError(msg)) => {
                format!("-ERR type: {msg}\r\n")
            }
            Self::Error(ResponseError::NotFound(key)) => {
                format!("-ERR not found: {key}\r\n")
            }
            Self::Error(ResponseError::InternalError(msg)) => {
                format!("-ERR internal: {msg}\r\n")
            }
            Self::Array(items) => {
                let mut s = format!("*{}\r\n", items.len());
                for item in items {
                    match item {
                        Some(data) => {
                            let val = String::from_utf8_lossy(data);
                            s.push_str(&format!("${}\r\n{val}\r\n", data.len()));
                        }
                        None => s.push_str("$-1\r\n"),
                    }
                }
                s
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pong() {
        assert_eq!(Response::Pong.serialize(), "+PONG\r\n");
    }

    #[test]
    fn ok() {
        assert_eq!(Response::Ok.serialize(), "+OK\r\n");
    }

    #[test]
    fn value_some() {
        let resp = Response::Value(Some(Bytes::from("hello")));
        assert_eq!(resp.serialize(), "+hello\r\n");
    }

    #[test]
    fn value_none() {
        assert_eq!(Response::Value(None).serialize(), "$-1\r\n");
    }

    #[test]
    fn integer() {
        assert_eq!(Response::Integer(42).serialize(), ":42\r\n");
        assert_eq!(Response::Integer(-1).serialize(), ":-1\r\n");
        assert_eq!(Response::Integer(0).serialize(), ":0\r\n");
    }

    #[test]
    fn error_unknown_command() {
        let resp = Response::Error(ResponseError::UnknownCommand("FOO".into()));
        assert_eq!(resp.serialize(), "-ERR unknown command 'FOO'\r\n");
    }

    #[test]
    fn error_syntax() {
        let resp = Response::Error(ResponseError::SyntaxError("bad input".into()));
        assert_eq!(resp.serialize(), "-ERR syntax: bad input\r\n");
    }

    #[test]
    fn error_type() {
        let resp = Response::Error(ResponseError::TypeError("not an integer".into()));
        assert_eq!(resp.serialize(), "-ERR type: not an integer\r\n");
    }

    #[test]
    fn error_not_found() {
        let resp = Response::Error(ResponseError::NotFound("mykey".into()));
        assert_eq!(resp.serialize(), "-ERR not found: mykey\r\n");
    }

    #[test]
    fn error_internal() {
        let resp = Response::Error(ResponseError::InternalError("oops".into()));
        assert_eq!(resp.serialize(), "-ERR internal: oops\r\n");
    }

    #[test]
    fn response_equality() {
        assert_eq!(Response::Pong, Response::Pong);
        assert_eq!(Response::Integer(1), Response::Integer(1));
        assert_ne!(Response::Pong, Response::Ok);
    }
}
