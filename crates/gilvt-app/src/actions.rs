use gpui::{actions, App, KeyBinding, Menu, MenuItem, SystemMenuType};

actions!(
    gilvt,
    [
        Quit,
        NewWindow,
        NewTab,
        ClosePane,
        CloseTab,
        SplitRight,
        SplitDown,
        FocusLeft,
        FocusRight,
        FocusUp,
        FocusDown,
        ResizeLeft,
        ResizeRight,
        ResizeUp,
        ResizeDown,
        ZoomPane,
        NextTab,
        PrevTab,
        Tab1,
        Tab2,
        Tab3,
        Tab4,
        Tab5,
        Tab6,
        Tab7,
        Tab8,
        Tab9,
        IncreaseFontSize,
        DecreaseFontSize,
        ResetFontSize,
        Copy,
        Paste,
        Find,
        FindFile,
        ClearScrollback,
        ScrollPageUp,
        ScrollPageDown,
        ScrollToTop,
        ScrollToBottom,
        ToggleSidebar,
        NextNeedsYou,
        PrevSession,
        NextSession,
        GroupByProject,
        GroupByStatus,
        RenameSession,
        MuteSession,
        ToggleInspector,
        InspectorProcess,
        InspectorArtifacts,
        InspectorConfig,
        OpenSessions,
        OpenCleanup,
        SessionCenterNeedsYou,
        SessionCenterReview,
        SessionCenterRunning,
        SessionCenterAll,
        ResumeBelow,
        NewAgent,
        OpenMonitor,
        ToggleCommandBar,
        NewAgentClaude,
        NewAgentCodex,
        NewAgentBelow,
        ResumeAll,
        EditorSave,
        EditorTogglePreview,
        EditorUndo,
        EditorRedo,
        EditorCut,
        EditorSelectAll,
        OpenExternalEditor,
        OpenSettings,
    ]
);

/// Key context set by `TerminalView`; terminal-only bindings are scoped to it.
pub const TERMINAL_CONTEXT: &str = "Terminal";
/// Key context set by `EditorView`; editor-only bindings are scoped to it.
pub const EDITOR_CONTEXT: &str = "Editor";
/// Key context of the `⌘P` palette.
pub const PALETTE_CONTEXT: &str = "FilePalette";
/// Key context of the `⌘⇧R` 会话 palette.
pub const SESSIONS_CONTEXT: &str = "SessionsPalette";
/// Key context of the `⌘⇧N` 新建 Agent panel.
pub const NEW_AGENT_CONTEXT: &str = "NewAgentPanel";
/// Key context of the sidebar's inline rename field.
pub const RENAME_CONTEXT: &str = "RenameField";
/// Key context of the monitor chat panel's input.
pub const CHAT_INPUT_CONTEXT: &str = "ChatInput";
/// Key context of the settings window's 「外观」 page (its search box takes ⌘V).
pub const APPEARANCE_CONTEXT: &str = "AppearancePage";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys(bindings());
}

/// Every key binding of the app (a pure list, so tests can check for clashes).
pub fn bindings() -> Vec<KeyBinding> {
    let t = Some(TERMINAL_CONTEXT);
    let palette = Some(PALETTE_CONTEXT);
    let rename = Some(RENAME_CONTEXT);
    let chat_input = Some(CHAT_INPUT_CONTEXT);
    let sessions = Some(SESSIONS_CONTEXT);
    let new_agent = Some(NEW_AGENT_CONTEXT);
    let editor = Some(EDITOR_CONTEXT);
    vec![
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-n", NewWindow, None),
        KeyBinding::new("cmd-,", OpenSettings, None),
        KeyBinding::new("cmd-t", NewTab, None),
        KeyBinding::new("cmd-w", ClosePane, None),
        KeyBinding::new("cmd-shift-w", CloseTab, None),
        KeyBinding::new("cmd-d", SplitRight, None),
        KeyBinding::new("cmd-shift-d", SplitDown, None),
        KeyBinding::new("cmd-alt-left", FocusLeft, None),
        KeyBinding::new("cmd-alt-right", FocusRight, None),
        KeyBinding::new("cmd-alt-up", FocusUp, None),
        KeyBinding::new("cmd-alt-down", FocusDown, None),
        KeyBinding::new("cmd-ctrl-left", ResizeLeft, None),
        KeyBinding::new("cmd-ctrl-right", ResizeRight, None),
        KeyBinding::new("cmd-ctrl-up", ResizeUp, None),
        KeyBinding::new("cmd-ctrl-down", ResizeDown, None),
        KeyBinding::new("cmd-shift-enter", ZoomPane, None),
        KeyBinding::new("cmd-}", NextTab, None),
        KeyBinding::new("cmd-{", PrevTab, None),
        KeyBinding::new("cmd-1", Tab1, None),
        KeyBinding::new("cmd-2", Tab2, None),
        KeyBinding::new("cmd-3", Tab3, None),
        KeyBinding::new("cmd-4", Tab4, None),
        KeyBinding::new("cmd-5", Tab5, None),
        KeyBinding::new("cmd-6", Tab6, None),
        KeyBinding::new("cmd-7", Tab7, None),
        KeyBinding::new("cmd-8", Tab8, None),
        KeyBinding::new("cmd-9", Tab9, None),
        KeyBinding::new("cmd-=", IncreaseFontSize, None),
        KeyBinding::new("cmd-+", IncreaseFontSize, None),
        KeyBinding::new("cmd--", DecreaseFontSize, None),
        KeyBinding::new("cmd-0", ResetFontSize, None),
        KeyBinding::new("cmd-b", ToggleSidebar, None),
        KeyBinding::new("cmd-shift-j", NextNeedsYou, None),
        KeyBinding::new("cmd-shift-up", PrevSession, None),
        KeyBinding::new("cmd-shift-down", NextSession, None),
        KeyBinding::new("cmd-i", ToggleInspector, None),
        // ⌥⌘1/2/3 like Xcode's inspectors: ⌘⇧3 is macOS's screenshot shortcut and never reached the app.
        KeyBinding::new("cmd-alt-1", InspectorProcess, None),
        KeyBinding::new("cmd-alt-2", InspectorArtifacts, None),
        KeyBinding::new("cmd-alt-3", InspectorConfig, None),
        KeyBinding::new("cmd-shift-r", OpenSessions, None),
        KeyBinding::new("cmd-shift-k", OpenCleanup, None),
        // ⌘⇧↩ is Zoom Pane elsewhere; in the 会话 palette it resumes below (the deeper context wins).
        KeyBinding::new("cmd-shift-enter", ResumeBelow, sessions),
        KeyBinding::new("cmd-1", SessionCenterNeedsYou, sessions),
        KeyBinding::new("cmd-2", SessionCenterReview, sessions),
        KeyBinding::new("cmd-3", SessionCenterRunning, sessions),
        KeyBinding::new("cmd-4", SessionCenterAll, sessions),
        KeyBinding::new("cmd-v", Paste, sessions),
        KeyBinding::new("cmd-shift-n", NewAgent, None),
        KeyBinding::new("cmd-shift-o", OpenMonitor, None),
        // The 监控官's bottom command bar (S2 §6.4): global, so a terminal or an agent's TUI never sees the chord.
        KeyBinding::new("cmd-shift-m", ToggleCommandBar, None),
        // In the 新建 Agent panel ⌘1 / ⌘2 pick the Agent (not a tab) and ⌘⇧↩ runs below (not Zoom Pane).
        KeyBinding::new("cmd-1", NewAgentClaude, new_agent),
        KeyBinding::new("cmd-2", NewAgentCodex, new_agent),
        KeyBinding::new("cmd-shift-enter", NewAgentBelow, new_agent),
        KeyBinding::new("cmd-v", Paste, new_agent),
        KeyBinding::new("cmd-c", Copy, t),
        KeyBinding::new("cmd-c", Copy, Some("Preview")),
        KeyBinding::new("cmd-v", Paste, t),
        KeyBinding::new("cmd-f", Find, t),
        KeyBinding::new("cmd-p", FindFile, t),
        KeyBinding::new("cmd-v", Paste, palette),
        KeyBinding::new("cmd-v", Paste, rename),
        KeyBinding::new("cmd-v", Paste, chat_input),
        KeyBinding::new("cmd-v", Paste, Some(APPEARANCE_CONTEXT)),
        KeyBinding::new("cmd-k", ClearScrollback, t),
        KeyBinding::new("shift-pageup", ScrollPageUp, t),
        KeyBinding::new("shift-pagedown", ScrollPageDown, t),
        KeyBinding::new("cmd-home", ScrollToTop, t),
        KeyBinding::new("cmd-end", ScrollToBottom, t),
        // Editor pane (E2a §5.5); none of these keys has a global binding, ⌘W / ⌘D / ⌘T / ⌘1–9 / ⌘Q stay global.
        KeyBinding::new("cmd-s", EditorSave, editor),
        KeyBinding::new("cmd-shift-v", EditorTogglePreview, editor),
        KeyBinding::new("cmd-z", EditorUndo, editor),
        KeyBinding::new("cmd-shift-z", EditorRedo, editor),
        KeyBinding::new("cmd-x", EditorCut, editor),
        KeyBinding::new("cmd-a", EditorSelectAll, editor),
        KeyBinding::new("cmd-c", Copy, editor),
        KeyBinding::new("cmd-v", Paste, editor),
        // ⌘O edits in the built-in editor now (a preview key); ⌘⌥O keeps the external editor.
        KeyBinding::new("cmd-alt-o", OpenExternalEditor, Some("Preview")),
        KeyBinding::new("cmd-alt-o", OpenExternalEditor, editor),
    ]
}

pub fn menus() -> Vec<Menu> {
    vec![
        Menu {
            name: "gilvt".into(),
            items: vec![
                MenuItem::action("New Window", NewWindow),
                MenuItem::separator(),
                MenuItem::action("设置…", OpenSettings),
                MenuItem::separator(),
                MenuItem::os_submenu("Services", SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action("Quit gilvt", Quit),
            ],
        },
        Menu {
            name: "Shell".into(),
            items: vec![
                MenuItem::action("New Tab", NewTab),
                MenuItem::action("Split Right", SplitRight),
                MenuItem::action("Split Down", SplitDown),
                MenuItem::separator(),
                MenuItem::action("Close Pane", ClosePane),
                MenuItem::action("Close Tab", CloseTab),
            ],
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::action("Copy", Copy),
                MenuItem::action("Paste", Paste),
                MenuItem::separator(),
                MenuItem::action("Find…", Find),
                MenuItem::action("Go to File…", FindFile),
                MenuItem::action("Clear Scrollback", ClearScrollback),
            ],
        },
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("Zoom Pane", ZoomPane),
                MenuItem::separator(),
                MenuItem::action("Bigger", IncreaseFontSize),
                MenuItem::action("Smaller", DecreaseFontSize),
                MenuItem::action("Actual Size", ResetFontSize),
            ],
        },
        Menu {
            name: "会话".into(),
            items: vec![
                MenuItem::action("新建 Agent…", NewAgent),
                MenuItem::action("监控官", OpenMonitor),
                MenuItem::action("监控官命令条", ToggleCommandBar),
                MenuItem::action("会话…", OpenSessions),
                MenuItem::action("清理…", OpenCleanup),
                MenuItem::action("Resume All Sessions", ResumeAll),
                MenuItem::separator(),
                MenuItem::action("跳到下一个需要你的会话", NextNeedsYou),
                MenuItem::action("上一个会话", PrevSession),
                MenuItem::action("下一个会话", NextSession),
                MenuItem::separator(),
                MenuItem::action("显示 / 隐藏会话栏", ToggleSidebar),
                MenuItem::action("会话栏：按项目", GroupByProject),
                MenuItem::action("会话栏：按状态", GroupByStatus),
                MenuItem::separator(),
                MenuItem::action("显示 / 隐藏检查器", ToggleInspector),
                MenuItem::action("检查器：过程", InspectorProcess),
                MenuItem::action("检查器：产物", InspectorArtifacts),
                MenuItem::action("检查器：配置", InspectorConfig),
                MenuItem::separator(),
                MenuItem::action("重命名当前会话…", RenameSession),
                MenuItem::action("静音当前会话的通知", MuteSession),
            ],
        },
    ]
}

#[cfg(test)]
mod tests {
    use gpui::{Action, Modifiers};

    use super::*;

    #[test]
    fn command_bar_has_its_own_global_chord() {
        let cmd_shift_m = Modifiers { platform: true, shift: true, ..Default::default() };
        let all = bindings();
        let hits: Vec<&KeyBinding> = all
            .iter()
            .filter(|b| b.keystrokes().len() == 1 && b.keystrokes()[0].inner().key == "m" && b.keystrokes()[0].inner().modifiers == cmd_shift_m)
            .collect();
        assert_eq!(hits.len(), 1, "⌘⇧M is bound exactly once");
        assert_eq!(hits[0].action().name(), ToggleCommandBar.name());
        assert!(hits[0].predicate().is_none(), "global: it must work from terminals, agent TUIs, editors and the monitor tab");
    }

    #[test]
    fn command_bar_is_in_the_session_menu() {
        let menus = menus();
        let session = menus.iter().find(|m| m.name.as_ref() == "会话").expect("会话 menu");
        assert!(session.items.iter().any(|i| matches!(i, MenuItem::Action { name, .. } if name.as_ref() == "监控官命令条")));
    }
}
