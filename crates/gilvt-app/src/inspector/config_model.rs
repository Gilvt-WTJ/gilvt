//! Pure view state of the 「配置」 tab (M5a+): which MCP / hook / memory rows are expanded and which
//! resource list (Skills, commands, sub-agents) the dialog shows. No gpui here, so it is unit-tested.

use std::collections::HashSet;

use gilvt_config::{Resource, Summary};

/// Cards whose rows expand in place (few rows, short details).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    Mcp,
    Hooks,
    Memory,
}

impl Section {
    pub fn name(self) -> &'static str {
        match self {
            Section::Mcp => "mcp",
            Section::Hooks => "hooks",
            Section::Memory => "memory",
        }
    }
}

/// Lists that open in a dialog (many rows, each with a description and a file).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    Skills,
    Commands,
    Subagents,
}

impl Group {
    pub const ALL: [Group; 3] = [Group::Skills, Group::Commands, Group::Subagents];

    pub fn name(self) -> &'static str {
        match self {
            Group::Skills => "skills",
            Group::Commands => "commands",
            Group::Subagents => "subagents",
        }
    }

    pub fn title(self, agent: gilvt_agent::AgentKind) -> &'static str {
        match (self, agent) {
            (Group::Skills, _) => "Skills",
            (Group::Commands, gilvt_agent::AgentKind::Codex) => "Prompts",
            (Group::Commands, _) => crate::i18n::text("斜杠命令", "Slash Commands"),
            (Group::Subagents, _) => crate::i18n::text("子 Agent", "Subagents"),
        }
    }

    pub fn items(self, summary: &Summary) -> &[Resource] {
        match self {
            Group::Skills => &summary.resources.skills,
            Group::Commands => &summary.resources.commands,
            Group::Subagents => &summary.resources.subagents,
        }
    }
}

/// Rows are keyed by name, not position, so a refresh that reorders or adds rows keeps them open.
#[derive(Debug, Default)]
pub struct Disclosure {
    open: HashSet<(Section, String)>,
    pub dialog: Option<Group>,
}

impl Disclosure {
    pub fn is_open(&self, section: Section, name: &str) -> bool {
        self.open.contains(&(section, name.to_string()))
    }

    pub fn toggle(&mut self, section: Section, name: &str) {
        let key = (section, name.to_string());
        if !self.open.remove(&key) {
            self.open.insert(key);
        }
    }

    /// Open rows as `section/name`, sorted, for the debug state.
    pub fn open_rows(&self) -> Vec<String> {
        let mut rows: Vec<String> = self.open.iter().map(|(s, n)| format!("{}/{n}", s.name())).collect();
        rows.sort();
        rows
    }

    /// A different session was focused: nothing stays open.
    pub fn reset(&mut self) {
        *self = Disclosure::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggling_twice_closes_and_rows_are_independent() {
        let mut d = Disclosure::default();
        d.toggle(Section::Mcp, "docs");
        assert!(d.is_open(Section::Mcp, "docs"));
        assert!(!d.is_open(Section::Hooks, "docs"), "same name in another card is a different row");
        d.toggle(Section::Mcp, "docs");
        assert!(!d.is_open(Section::Mcp, "docs"));
    }

    #[test]
    fn reset_closes_rows_and_the_dialog() {
        let mut d = Disclosure::default();
        d.toggle(Section::Memory, "/a/CLAUDE.md");
        d.dialog = Some(Group::Skills);
        assert_eq!(d.open_rows(), vec!["memory//a/CLAUDE.md".to_string()]);
        d.reset();
        assert!(d.open_rows().is_empty());
        assert_eq!(d.dialog, None);
    }
}
