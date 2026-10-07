//! The session's TODO list (M3b spec §3.3): Claude `TodoWrite` (the latest list), `TaskCreate` / `TaskUpdate`
//! (accumulated), Codex `update_plan` (the latest plan).

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::event::str_field;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanState {
    Done,
    Active,
    Todo,
}

impl PlanState {
    /// `completed` / `in_progress` / anything else (`pending`).
    fn from_status(status: &str) -> PlanState {
        match status {
            "completed" => PlanState::Done,
            "in_progress" => PlanState::Active,
            _ => PlanState::Todo,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanItem {
    pub text: String,
    pub state: PlanState,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Plan {
    items: Vec<PlanItem>,
    /// Claude task ids, parallel to `items` (None for TodoWrite / update_plan entries).
    ids: Vec<Option<String>>,
    /// TaskCreate tool-use id → the id given to its task before the result names the real one.
    provisional: HashMap<String, String>,
    next_task: u64,
    /// Tool calls already applied (a re-read transcript must not add tasks twice).
    applied: HashSet<String>,
}

impl Plan {
    pub(super) fn items(&self) -> &[PlanItem] {
        &self.items
    }

    /// A tool call from the transcript; other tools are ignored.
    pub(super) fn apply_call(&mut self, id: &str, tool: &str, input: &Value) {
        if !matches!(tool, "TodoWrite" | "TaskCreate" | "TaskUpdate" | "update_plan") {
            return;
        }
        if !id.is_empty() && !self.applied.insert(id.to_string()) {
            return;
        }
        match tool {
            "TodoWrite" => self.replace(input.get("todos"), "content"),
            "update_plan" => self.replace(input.get("plan"), "step"),
            "TaskCreate" => self.create(id, input),
            _ => self.update(input),
        }
    }

    /// The whole list is replaced; `key` names the entry text.
    fn replace(&mut self, list: Option<&Value>, key: &str) {
        let Some(list) = list.and_then(Value::as_array) else { return };
        self.items = list
            .iter()
            .filter_map(|e| {
                let text = str_field(e, key)?.to_string();
                Some(PlanItem { text, state: PlanState::from_status(str_field(e, "status").unwrap_or("")) })
            })
            .collect();
        self.ids = vec![None; self.items.len()];
    }

    /// Claude numbers tasks 1, 2, … per session; the result confirms the number ([`Plan::task_created`]).
    fn create(&mut self, tool_use_id: &str, input: &Value) {
        let Some(text) = str_field(input, "subject").or_else(|| str_field(input, "description")) else { return };
        self.next_task = self.next_task.max(1);
        let id = self.next_task.to_string();
        self.next_task += 1;
        self.provisional.insert(tool_use_id.to_string(), id.clone());
        self.items.push(PlanItem { text: text.to_string(), state: PlanState::Todo });
        self.ids.push(Some(id));
    }

    /// The TaskCreate result named its task id.
    pub(super) fn task_created(&mut self, tool_use_id: &str, task_id: &str) {
        let Some(provisional) = self.provisional.remove(tool_use_id) else { return };
        if let Some(slot) = self.ids.iter_mut().find(|i| i.as_deref() == Some(&provisional)) {
            *slot = Some(task_id.to_string());
        }
        if let Ok(n) = task_id.parse::<u64>() {
            self.next_task = self.next_task.max(n + 1);
        }
    }

    fn update(&mut self, input: &Value) {
        let Some(task) = str_field(input, "taskId") else { return };
        let Some(i) = self.ids.iter().position(|id| id.as_deref() == Some(task)) else { return };
        match str_field(input, "status") {
            Some("deleted") => {
                self.items.remove(i);
                self.ids.remove(i);
                return;
            }
            Some(status) => self.items[i].state = PlanState::from_status(status),
            None => {}
        }
        if let Some(subject) = str_field(input, "subject") {
            self.items[i].text = subject.to_string();
        }
    }
}

/// The task id a TaskCreate result names: `toolUseResult.task.id`, else `Task #N created …` in the text.
pub(super) fn created_task_id(tool_use_result: Option<&Value>, text: &str) -> Option<String> {
    if let Some(id) = tool_use_result.and_then(|r| r.pointer("/task/id")).and_then(Value::as_str) {
        return Some(id.to_string());
    }
    let rest = text.strip_prefix("Task #")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    (!digits.is_empty()).then_some(digits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn states(plan: &Plan) -> Vec<(&str, PlanState)> {
        plan.items().iter().map(|i| (i.text.as_str(), i.state)).collect()
    }

    #[test]
    fn todo_write_replaces_the_list() {
        let mut plan = Plan::default();
        let todos = |a: &str, b: &str| {
            json!({"todos": [{"content": "write file", "status": a, "activeForm": "Writing file"},
                             {"content": "check file", "status": b, "activeForm": "Checking file"}]})
        };
        plan.apply_call("t1", "TodoWrite", &todos("in_progress", "pending"));
        assert_eq!(states(&plan), [("write file", PlanState::Active), ("check file", PlanState::Todo)]);
        plan.apply_call("t2", "TodoWrite", &todos("completed", "in_progress"));
        assert_eq!(states(&plan), [("write file", PlanState::Done), ("check file", PlanState::Active)]);
        plan.apply_call("t3", "TodoWrite", &json!({"todos": []}));
        assert!(plan.items().is_empty());
    }

    #[test]
    fn tasks_accumulate() {
        let mut plan = Plan::default();
        plan.apply_call("c1", "TaskCreate", &json!({"subject": "a"}));
        plan.apply_call("c2", "TaskCreate", &json!({"subject": "b"}));
        plan.apply_call("c2", "TaskCreate", &json!({"subject": "b"}));
        plan.task_created("c2", "7");
        plan.apply_call("u1", "TaskUpdate", &json!({"taskId": "7", "status": "in_progress", "subject": "b2"}));
        plan.apply_call("u2", "TaskUpdate", &json!({"taskId": "1", "status": "completed"}));
        assert_eq!(states(&plan), [("a", PlanState::Done), ("b2", PlanState::Active)]);
        plan.apply_call("c3", "TaskCreate", &json!({"subject": "c"}));
        plan.apply_call("u3", "TaskUpdate", &json!({"taskId": "8", "status": "deleted"}));
        plan.apply_call("u4", "TaskUpdate", &json!({"taskId": "99", "status": "completed"}));
        assert_eq!(states(&plan), [("a", PlanState::Done), ("b2", PlanState::Active)]);
    }

    #[test]
    fn created_ids() {
        assert_eq!(created_task_id(Some(&json!({"task": {"id": "3"}})), ""), Some("3".into()));
        assert_eq!(created_task_id(None, "Task #12 created successfully: x"), Some("12".into()));
        assert_eq!(created_task_id(None, "created"), None);
    }
}
