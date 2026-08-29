use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentAction {
    pub action_type: String,
    pub target: Option<String>,
    pub parameters: std::collections::HashMap<String, String>,
}

pub struct IntentAgent {
    history: Vec<IntentAction>,
    max_history: usize,
}

impl IntentAgent {
    pub fn new() -> Self {
        Self {
            history: Vec::new(),
            max_history: 100,
        }
    }

    pub fn parse_intent(&mut self, intent_json: &str) -> Result<IntentAction> {
        let action: IntentAction = serde_json::from_str(intent_json)?;
        self.history.push(action.clone());

        if self.history.len() > self.max_history {
            self.history.remove(0);
        }

        tracing::debug!(action = %action.action_type, "Parsed intent");
        Ok(action)
    }

    pub fn predict_next(&self) -> Option<String> {
        if self.history.len() < 2 {
            return None;
        }

        let last = &self.history[self.history.len() - 1];
        let second_last = &self.history[self.history.len() - 2];

        if last.action_type == second_last.action_type {
            Some(last.action_type.clone())
        } else {
            None
        }
    }

    pub fn history(&self) -> &[IntentAction] {
        &self.history
    }

    pub fn clear_history(&mut self) {
        self.history.clear();
    }
}

impl Default for IntentAgent {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_intent() {
        let mut agent = IntentAgent::new();
        let json = r#"{"action_type": "click", "target": "button", "parameters": {}}"#;
        let action = agent.parse_intent(json).unwrap();
        assert_eq!(action.action_type, "click");
        assert_eq!(agent.history().len(), 1);
    }

    #[test]
    fn test_parse_intent_with_parameters() {
        let mut agent = IntentAgent::new();
        let json = r#"{"action_type": "type", "target": "input", "parameters": {"text": "hello"}}"#;
        let action = agent.parse_intent(json).unwrap();
        assert_eq!(action.action_type, "type");
        assert_eq!(action.parameters.get("text").unwrap(), "hello");
    }

    #[test]
    fn test_predict_next_same_action() {
        let mut agent = IntentAgent::new();
        agent
            .parse_intent(r#"{"action_type": "click", "target": null, "parameters": {}}"#)
            .unwrap();
        agent
            .parse_intent(r#"{"action_type": "click", "target": null, "parameters": {}}"#)
            .unwrap();
        assert_eq!(agent.predict_next(), Some("click".to_string()));
    }

    #[test]
    fn test_predict_next_different_actions() {
        let mut agent = IntentAgent::new();
        agent
            .parse_intent(r#"{"action_type": "click", "target": null, "parameters": {}}"#)
            .unwrap();
        agent
            .parse_intent(r#"{"action_type": "type", "target": null, "parameters": {}}"#)
            .unwrap();
        assert!(agent.predict_next().is_none());
    }

    #[test]
    fn test_predict_next_too_few() {
        let mut agent = IntentAgent::new();
        agent
            .parse_intent(r#"{"action_type": "click", "target": null, "parameters": {}}"#)
            .unwrap();
        assert!(agent.predict_next().is_none());
    }

    #[test]
    fn test_clear_history() {
        let mut agent = IntentAgent::new();
        agent
            .parse_intent(r#"{"action_type": "click", "target": null, "parameters": {}}"#)
            .unwrap();
        assert_eq!(agent.history().len(), 1);
        agent.clear_history();
        assert_eq!(agent.history().len(), 0);
    }

    #[test]
    fn test_history_bounded() {
        let mut agent = IntentAgent::new();
        for _ in 0..150 {
            agent
                .parse_intent(r#"{"action_type": "click", "target": null, "parameters": {}}"#)
                .unwrap();
        }
        assert!(agent.history().len() <= 100);
    }

    #[test]
    fn test_parse_invalid_json() {
        let mut agent = IntentAgent::new();
        let result = agent.parse_intent("not json");
        assert!(result.is_err());
    }
}
