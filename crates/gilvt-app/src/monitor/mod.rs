//! The 「◎ 监控官」 tab (S1): every window's agent sessions and plain terminals as cards, drawn by the
//! `Workspace` like the sidebar (it reads every window). Read-only: nothing is written to any pane.

pub mod chat;
pub mod chat_md;
pub mod chat_input;
pub mod chat_model;
pub mod chat_view;
pub mod command_bar;
pub mod gather;
pub mod model;
pub mod summaries;
pub mod tools;
pub mod view;

/// The monitor tab's pane: its keyboard focus and view state. The cards are drawn by the `Workspace`.
#[derive(Clone)]
pub struct MonitorPane {
    pub focus: gpui::FocusHandle,
    pub ui: MonitorUi,
    /// The chat panel's input (made with the pane; drawn only while the 监控官 is on).
    pub chat_input: gpui::Entity<chat_input::ChatInput>,
    /// The tab's width in the last frame (logical px; 0 before the first): below `chat_view::NARROW_WIDTH` the
    /// chat is a rail.
    pub width: std::rc::Rc<std::cell::Cell<f32>>,
    pub chat_scroll: gpui::ScrollHandle,
    /// The chat revision the messages were last scrolled to the bottom for.
    pub chat_seen: std::rc::Rc<std::cell::Cell<u64>>,
    /// The number of questions in the chat at that revision: a new one scrolls to the bottom even when the user
    /// had scrolled up.
    pub chat_questions: std::rc::Rc<std::cell::Cell<usize>>,
}

/// Which group the wall shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    Only(model::Group),
}

/// The tab's view state (kept in its pane, not saved).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MonitorUi {
    pub filter: Filter,
    /// Key of the selected card (`Card::key`).
    pub selected: Option<String>,
    /// Key of the card whose 补课 list is open (one at a time).
    pub expanded: Option<String>,
    /// 已结束 is open.
    pub ended_open: bool,
    /// ⇥ in a wide tab: the chat is a rail until it is clicked.
    pub chat_collapsed: bool,
    /// The chat overlay over a narrow tab is open.
    pub chat_open: bool,
}

impl MonitorUi {
    /// After `model` was built from this state: a filter that fell back to 全部 (its group emptied) is reset,
    /// so the wall does not jump back to that group alone when it fills again.
    pub fn settle(&mut self, model: &model::MonitorModel) {
        self.filter = model.filter;
    }
}
