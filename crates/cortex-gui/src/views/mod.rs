//! The parts of the window. Each view draws itself from state it is handed
//! and reports what the person did; none of them talks to the agent.

pub mod composer;
pub mod conversation;
pub mod files;
pub mod icons;
pub mod insights;
pub mod menu;
pub mod prose;
pub mod settings;
pub mod sidebar;
pub mod stats_strip;
pub mod tool_card;
pub mod top_bar;
pub mod typeset;

/// What the middle of the window shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Conversation,

    /// Tokens used, and whether the advisers help.
    Insights,

    Settings,
}

/// Which kind of session the sidebar lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    /// Sessions that belong to no project.
    Chats,

    /// Project folders and the sessions in them.
    Code,
}
