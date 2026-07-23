use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum YapperError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("audio error: {0}")]
    Audio(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid state: {0}")]
    State(String),
}

// Tauri commands need serializable errors; the UI only shows the message.
impl Serialize for YapperError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}
