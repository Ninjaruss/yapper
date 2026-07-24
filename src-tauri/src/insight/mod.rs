//! Slow-lane abstraction. One engine per session; called on a relaxed cadence.

pub mod guard;
pub mod llama;
pub mod prompt;
pub mod worker;

use crate::error::YapperError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OutlineEntry {
    pub label: String, // short topic label, user's own words preferred
    pub status: OutlineStatus,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutlineStatus {
    Covered,
    Current,
    IntentUntouched,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InsightUpdate {
    pub outline: Vec<OutlineEntry>, // full replacement snapshot, ≤10 entries
    pub question: Option<String>,   // at most one; None = nothing worth asking
    pub sparked_by: Option<String>, // verbatim transcript phrase that sparked the question
    pub wrapup_ready: bool,         // model's judgment of "circling / natural close available"
    pub shine: bool,                // the most recent stretch went notably deep/personal
}

/// The request is prebuilt text (prompt.rs owns formatting) so engines stay dumb.
pub trait InsightEngine: Send {
    fn insight(&mut self, prompt: &str) -> Result<String, YapperError>; // raw model text out
}

pub struct MockInsight {
    script: std::collections::VecDeque<String>,
}
impl MockInsight {
    pub fn new(script: Vec<String>) -> Self {
        Self {
            script: script.into(),
        }
    }
}
impl InsightEngine for MockInsight {
    fn insight(&mut self, _prompt: &str) -> Result<String, YapperError> {
        Ok(self.script.pop_front().unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_insight_echoes_script() {
        let mut mock = MockInsight::new(vec![
            "first response".to_string(),
            "second response".to_string(),
        ]);

        assert_eq!(mock.insight("prompt 1").unwrap(), "first response");
        assert_eq!(mock.insight("prompt 2").unwrap(), "second response");
    }

    #[test]
    fn mock_insight_empty_when_exhausted() {
        let mut mock = MockInsight::new(vec!["one".to_string()]);

        // First call consumes the only item
        assert_eq!(mock.insight("prompt").unwrap(), "one");

        // Subsequent calls return empty string (default)
        assert_eq!(mock.insight("prompt").unwrap(), "");
        assert_eq!(mock.insight("prompt").unwrap(), "");
    }
}
