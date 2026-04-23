use crate::domain::command::Command;
use crate::protocol::response::{Response, ResponseError};
use crate::storage::engine::KvEngine;

pub fn dispatch(engine: &dyn KvEngine, cmd: Command) -> Response {
    match cmd {
        Command::Ping => Response::Pong,
        Command::Get { key } => match engine.get(&key) {
            Ok(Some(entry)) => Response::Value(Some(entry.data)),
            Ok(None) => Response::Value(None),
            Err(e) => Response::Error(ResponseError::InternalError(e.to_string())),
        },
        Command::Set { key, value, ttl } => {
            let entry = match ttl {
                Some(dur) => crate::domain::value::ValueEntry::with_ttl(value, dur),
                None => crate::domain::value::ValueEntry::new(value),
            };
            match engine.set(key, entry) {
                Ok(()) => Response::Ok,
                Err(e) => Response::Error(ResponseError::InternalError(e.to_string())),
            }
        }
        Command::Del { key } => match engine.del(&key) {
            Ok(existed) => Response::Integer(if existed { 1 } else { 0 }),
            Err(e) => Response::Error(ResponseError::InternalError(e.to_string())),
        },
        Command::Incr { key } => match engine.incr(&key) {
            Ok(n) => Response::Integer(n),
            Err(e) => Response::Error(ResponseError::TypeError(e.to_string())),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::value::ValueEntry;
    use bytes::Bytes;
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct MockEngine {
        map: Mutex<HashMap<String, ValueEntry>>,
    }

    impl MockEngine {
        fn new() -> Self {
            Self {
                map: Mutex::new(HashMap::new()),
            }
        }
    }

    impl KvEngine for MockEngine {
        fn get(&self, key: &str) -> Result<Option<ValueEntry>, crate::domain::errors::EngineError> {
            let map = self.map.lock().unwrap();
            Ok(map.get(key).map(|e| ValueEntry::new(e.data.clone())))
        }

        fn set(
            &self,
            key: String,
            value: ValueEntry,
        ) -> Result<(), crate::domain::errors::EngineError> {
            let mut map = self.map.lock().unwrap();
            map.insert(key, value);
            Ok(())
        }

        fn del(&self, key: &str) -> Result<bool, crate::domain::errors::EngineError> {
            let mut map = self.map.lock().unwrap();
            Ok(map.remove(key).is_some())
        }

        fn incr(&self, key: &str) -> Result<i64, crate::domain::errors::EngineError> {
            let mut map = self.map.lock().unwrap();
            let entry = map
                .entry(key.to_string())
                .or_insert_with(|| ValueEntry::new(Bytes::from("0")));
            let val: i64 = String::from_utf8_lossy(&entry.data)
                .parse()
                .map_err(|_| crate::domain::errors::EngineError::NotAnInteger(key.to_string()))?;
            let new_val = val + 1;
            entry.data = Bytes::from(new_val.to_string());
            Ok(new_val)
        }
    }

    #[test]
    fn dispatch_ping() {
        let engine = MockEngine::new();
        let resp = dispatch(&engine, Command::Ping);
        assert_eq!(resp, Response::Pong);
    }

    #[test]
    fn dispatch_set_and_get() {
        let engine = MockEngine::new();
        let resp = dispatch(
            &engine,
            Command::Set {
                key: "k".into(),
                value: Bytes::from("v"),
                ttl: None,
            },
        );
        assert_eq!(resp, Response::Ok);

        let resp = dispatch(&engine, Command::Get { key: "k".into() });
        assert_eq!(resp, Response::Value(Some(Bytes::from("v"))));
    }

    #[test]
    fn dispatch_get_missing() {
        let engine = MockEngine::new();
        let resp = dispatch(&engine, Command::Get { key: "missing".into() });
        assert_eq!(resp, Response::Value(None));
    }

    #[test]
    fn dispatch_del() {
        let engine = MockEngine::new();
        dispatch(
            &engine,
            Command::Set {
                key: "k".into(),
                value: Bytes::from("v"),
                ttl: None,
            },
        );
        let resp = dispatch(&engine, Command::Del { key: "k".into() });
        assert_eq!(resp, Response::Integer(1));

        let resp = dispatch(&engine, Command::Del { key: "k".into() });
        assert_eq!(resp, Response::Integer(0));
    }

    #[test]
    fn dispatch_incr() {
        let engine = MockEngine::new();
        let resp = dispatch(&engine, Command::Incr { key: "c".into() });
        assert_eq!(resp, Response::Integer(1));
        let resp = dispatch(&engine, Command::Incr { key: "c".into() });
        assert_eq!(resp, Response::Integer(2));
    }
}
