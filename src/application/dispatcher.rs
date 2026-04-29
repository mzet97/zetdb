use crate::domain::command::Command;
use crate::observability::metrics::{self, CommandType};
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
        Command::Info => {
            let m = metrics::metrics();
            let uptime = m.uptime_secs();
            let info = format!(
                "# Server\r\nzetdb_version:0.1.0\r\nuptime_in_seconds:{uptime}\r\n\r\n\
                 # Clients\r\nconnected_clients:{}\r\ntotal_connections:{}\r\n\r\n\
                 # Stats\r\ntotal_commands:{}\r\n\
                 cmd_ping:{}\r\ncmd_get:{}\r\ncmd_set:{}\r\n\
                 cmd_del:{}\r\ncmd_incr:{}\r\ncmd_info:{}\r\ncmd_dbsize:{}\r\n\
                 keyspace_hits:{}\r\nkeyspace_misses:{}\r\nerrors_total:{}\r\n\r\n\
                 # Keyspace\r\ndb0:keys={}\r\n",
                m.connections_active
                    .load(std::sync::atomic::Ordering::Relaxed),
                m.connections_total
                    .load(std::sync::atomic::Ordering::Relaxed),
                m.commands_total.load(std::sync::atomic::Ordering::Relaxed),
                m.command_count(CommandType::Ping),
                m.command_count(CommandType::Get),
                m.command_count(CommandType::Set),
                m.command_count(CommandType::Del),
                m.command_count(CommandType::Incr),
                m.command_count(CommandType::Info),
                m.command_count(CommandType::DbSize),
                m.keyspace_hits.load(std::sync::atomic::Ordering::Relaxed),
                m.keyspace_misses.load(std::sync::atomic::Ordering::Relaxed),
                m.errors_total.load(std::sync::atomic::Ordering::Relaxed),
                engine.len(),
            );
            Response::Value(Some(bytes::Bytes::from(info)))
        }
        Command::DbSize => Response::Integer(engine.len() as i64),
        Command::Exists { key } => Response::Integer(if engine.exists(&key) { 1 } else { 0 }),
        Command::Ttl { key } => Response::Integer(engine.ttl_secs(&key)),
        Command::Expire { key, seconds } => {
            Response::Integer(if engine.expire(&key, seconds) { 1 } else { 0 })
        }
        Command::FlushDb => {
            engine.clear();
            Response::Ok
        }
        Command::Keys => {
            let keys: Vec<Option<bytes::Bytes>> = engine
                .keys()
                .into_iter()
                .map(|k| Some(bytes::Bytes::from(k)))
                .collect();
            Response::Array(keys)
        }
        Command::Mget { keys } => {
            let key_refs: Vec<&str> = keys.iter().map(|s| s.as_str()).collect();
            let results = engine.mget(&key_refs);
            let items: Vec<Option<bytes::Bytes>> = results
                .into_iter()
                .map(|opt| opt.map(|entry| entry.data))
                .collect();
            Response::Array(items)
        }
        Command::Mset { pairs } => {
            for (key, value) in pairs {
                let entry = crate::domain::value::ValueEntry::new(value);
                if let Err(e) = engine.set(key, entry) {
                    return Response::Error(ResponseError::InternalError(e.to_string()));
                }
            }
            Response::Ok
        }
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

        fn len(&self) -> usize {
            self.map.lock().unwrap().len()
        }

        fn exists(&self, key: &str) -> bool {
            let map = self.map.lock().unwrap();
            map.contains_key(key)
        }

        fn ttl_secs(&self, _key: &str) -> i64 {
            -1 // Mock: no TTL support
        }

        fn expire(&self, _key: &str, _seconds: u64) -> bool {
            false // Mock: no TTL support
        }

        fn clear(&self) {
            self.map.lock().unwrap().clear();
        }

        fn keys(&self) -> Vec<String> {
            let map = self.map.lock().unwrap();
            map.keys().cloned().collect()
        }

        fn mget(&self, keys: &[&str]) -> Vec<Option<ValueEntry>> {
            let map = self.map.lock().unwrap();
            keys.iter()
                .map(|key| map.get(*key).map(|e| ValueEntry::new(e.data.clone())))
                .collect()
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
        let resp = dispatch(
            &engine,
            Command::Get {
                key: "missing".into(),
            },
        );
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

    #[test]
    fn dispatch_mget() {
        let engine = MockEngine::new();
        dispatch(
            &engine,
            Command::Set {
                key: "a".into(),
                value: Bytes::from("1"),
                ttl: None,
            },
        );
        dispatch(
            &engine,
            Command::Set {
                key: "b".into(),
                value: Bytes::from("2"),
                ttl: None,
            },
        );
        let resp = dispatch(
            &engine,
            Command::Mget {
                keys: vec!["a".into(), "missing".into(), "b".into()],
            },
        );
        assert_eq!(
            resp,
            Response::Array(vec![Some(Bytes::from("1")), None, Some(Bytes::from("2")),])
        );
    }

    #[test]
    fn dispatch_mset() {
        let engine = MockEngine::new();
        let resp = dispatch(
            &engine,
            Command::Mset {
                pairs: vec![
                    ("k1".into(), Bytes::from("v1")),
                    ("k2".into(), Bytes::from("v2")),
                ],
            },
        );
        assert_eq!(resp, Response::Ok);

        assert_eq!(
            dispatch(&engine, Command::Get { key: "k1".into() }),
            Response::Value(Some(Bytes::from("v1")))
        );
        assert_eq!(
            dispatch(&engine, Command::Get { key: "k2".into() }),
            Response::Value(Some(Bytes::from("v2")))
        );
    }
}
