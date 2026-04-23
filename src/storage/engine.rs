use crate::domain::errors::EngineError;
use crate::domain::value::ValueEntry;

pub trait KvEngine: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<ValueEntry>, EngineError>;
    fn set(&self, key: String, value: ValueEntry) -> Result<(), EngineError>;
    fn del(&self, key: &str) -> Result<bool, EngineError>;
    fn incr(&self, key: &str) -> Result<i64, EngineError>;
    fn len(&self) -> usize;
}
