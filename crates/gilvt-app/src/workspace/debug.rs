//! The window's part of `gilvt debug state` (spec §3.2 `windows[]`): gathers the sidebar model, tabs and
//! panes, the open menu and overlay and the inspector, with the rects recorded in the latest frame, and
//! hands them to the pure mappers of `debug_state::map`. Read-only: nothing is written to a PTY and no
//! state changes; it runs on `&Workspace` outside any update, so other windows can be read too.

use std::time::Instant;

use gilvt_agent::SessionKey;
use gpui::{AnyWindowHandle, App, Modifiers};

use super::{marks, workspaces, PaneView, Tab, Workspace};
use crate::agents::{describe_process, Agents};
use crate::debug_state::rects::{self, Rect4, RectId};
use crate::debug_state::{self as ds, map, timeline, Overlay};
use crate::launcher::Location;
use crate::pane_tree::PaneId;
use crate::persist::snapshot::WindowSnap;
use crate::theme::AppSettings;

/// What only the window itself knows (read in a short `update` before the workspace is read).
pub struct WindowInfo {
    pub id: Option<u64>,
    pub key: bool,
    pub title: String,
    pub modifiers: Modifiers,
    /// The titlebar's height: how far below the window frame's top gpui's content view (the origin of its
    /// bounds) starts. Added to every rect, which are reported in window-frame coordinates.
    pub titlebar: f32,
    /// This window's `Workspace::snapshot` (needs the window itself, so the caller takes it).
    pub layout: WindowSnap,
    /// The command bar's input has the keyboard.
    pub command_bar_focused: bool,
}

impl Workspace {
    /// The snapshot of this window (`me`), with `tail` rows of every terminal's screen.
    pub fn debug_window(&self, me: AnyWindowHandle, info: WindowInfo, tail: usize, cx: &App) -> ds::WindowState {
        let recorded = rects::of_window(me.window_id(), cx);
        let rect = |id: RectId| recorded.get(&id).map(|r| rects::in_frame(*r, info.titlebar));
        let agents = cx.global::<Agents>();
        let registry = agents.registry();
        let model = (!self.sidebar.hidden).then(|| crate::sidebar::view::model_for(self, me, cx));
        let trash: Option<Vec<SessionKey>> =
            self.ended_confirm.as_ref().map(|c| c.entries.iter().map(|e| (e.agent, e.session_id.clone())).collect());
        let mut sidebar = map::sidebar(model.as_ref(), self.sidebar.grouping, self.renaming_key(), &|k| registry.get(k), &rect, trash.as_deref(), &|k| registry.get(k).and_then(|s| agents.git(s)).cloned());
        // The entry's number is the one the sidebar drew in its latest frame.
        sidebar.review_entry = sidebar.visible.then(|| ds::ReviewEntry {
            count: cx.try_global::<crate::session_center::pending::PendingCache>().map_or(0, |cache| cache.last()),
            rect: rect(RectId::SidebarReviewEntry),
        });
        let tabs = self.tabs.iter().enumerate().map(|(ti, tab)| self.debug_tab(ti, tab, ti == self.active, tail, info.titlebar, &rect, cx)).collect();
        let command_bar = self.debug_command_bar(info.command_bar_focused, &rect, cx);
        ds::WindowState {
            id: info.id,
            key: info.key,
            title: info.title,
            sidebar,
            tabs,
            tab_bar: rect(RectId::TabBar),
            context_menu: self.debug_menu(&rect, cx),
            overlay: self.debug_overlay(info.modifiers, &rect, cx),
            inspector: self.debug_inspector(&rect, cx),
            error_banner: self.error.clone(),
            install_banner: crate::install_notice::debug(&rect, cx),
            dividers: match self.inspector.hidden {
                true => Vec::new(),
                false => vec![ds::Divider { between: "center|inspector", rect: rect(RectId::InspectorEdge) }],
            },
            layout: info.layout,
            editors: self.debug_editors(&rect, cx),
            close_confirm: self.close_confirm.as_ref().map(|c| map::close_confirm(c.action.name(), &c.items, &c.dirty)),
            monitor: self.debug_monitor(&rect, cx),
            command_bar,
        }
    }

    /// The window's command bar; `None` while the 监控官 is off.
    pub(crate) fn debug_command_bar(&self, focused: bool, rect: &dyn Fn(RectId) -> Option<Rect4>, cx: &App) -> Option<ds::CommandBarState> {
        cx.global::<AppSettings>().0.monitor.enabled.then(|| {
            let input = self.command_bar.input.read(cx);
            crate::monitor::command_bar::debug_command_bar(&crate::monitor::chat::view(cx), self.command_bar.expanded, focused, input.draft(), &input.matches(), rect)
        })
    }

    /// The monitor tab's card wall as drawn in the latest frame (None unless this window drew one), walked in
    /// `monitor::view::render`'s order (`chips` / `drawn`) so the `RectId` numbers match.
    fn debug_monitor(&self, rect: &dyn Fn(RectId) -> Option<Rect4>, cx: &App) -> Option<ds::MonitorState> {
        use crate::monitor::view::{card_lines, catchup_texts, chips, drawn};
        use crate::monitor::{model::Card, Filter};
        let model = self.monitor_model()?;
        let pane = self.monitor_pane()?;
        let Some(PaneView::Monitor(m)) = self.panes.get(&pane) else { return None };
        let ui = &m.ui;
        let mut groups = Vec::new();
        let mut cards = Vec::new();
        for d in drawn(model) {
            groups.push(ds::MonitorGroup { name: d.view.group.id(), count: d.view.cards.len(), collapsed: d.view.collapsed, rect: rect(RectId::MonitorGroup(d.index)) });
            for (n, c) in d.cards {
                let key = c.key();
                let (title, status_line, meta_line) = card_lines(c);
                let expanded = ui.expanded.as_deref() == Some(key.as_str());
                cards.push(ds::MonitorCard {
                    selected: ui.selected.as_deref() == Some(key.as_str()),
                    key,
                    kind: match c {
                        Card::Agent(_) => "agent",
                        Card::Terminal(_) => "terminal",
                    },
                    group: d.view.group.id(),
                    title,
                    status_line,
                    meta_line,
                    expanded,
                    rect: rect(RectId::MonitorCard(n)),
                    jump: rect(RectId::MonitorJump(n)),
                    catchup: rect(RectId::MonitorCatchup(n)),
                    rows: match expanded {
                        true => catchup_texts(c).into_iter().enumerate().map(|(j, text)| ds::MonitorRow { text, rect: rect(RectId::MonitorCatchupRow(n, j)) }).collect(),
                        false => Vec::new(),
                    },
                    summary: c.summary().map(|s| {
                        let (goal, recent) = crate::monitor::view::summary_texts(s);
                        ds::MonitorSummary { state: s.state, header: s.header.clone(), goal, recent, rect: rect(RectId::MonitorSummary(n)) }
                    }),
                    resummarize: rect(RectId::MonitorResummarize(n)),
                    ask: rect(RectId::MonitorAsk(n)),
                });
            }
        }
        Some(ds::MonitorState {
            pane,
            filter: match model.filter {
                Filter::All => "all".to_string(),
                Filter::Only(g) => g.id().to_string(),
            },
            needs_you: model.needs_you(),
            selected: ui.selected.clone(),
            expanded: ui.expanded.clone(),
            chips: chips(model)
                .into_iter()
                .enumerate()
                .map(|(i, (filter, label))| ds::MonitorChip { label, active: model.filter == filter, rect: rect(RectId::MonitorFilter(i)) })
                .collect(),
            groups,
            cards,
            chat: cx.global::<AppSettings>().0.monitor.enabled.then(|| {
                let input = m.chat_input.read(cx);
                let mode = crate::monitor::chat_view::panel_mode(m.width.get(), ui.chat_collapsed, ui.chat_open);
                crate::monitor::chat_view::debug_chat(&crate::monitor::chat::view(cx), mode, input.draft(), &input.matches(), rect)
            }),
        })
    }

    /// The editor panes of every tab, in tab then layout order.
    fn debug_editors(&self, rect: &dyn Fn(RectId) -> Option<Rect4>, cx: &App) -> Vec<ds::EditorState> {
        let mut out = Vec::new();
        for (ti, tab) in self.tabs.iter().enumerate() {
            for id in tab.tree.panes() {
                let Some(PaneView::Editor(v)) = self.panes.get(&id) else { continue };
                let v = v.read(cx);
                let pos = v.model.caret_position();
                let buf = &v.model.buf;
                let (menu, menu_kind) = map::editor_menu(v.placed_menu(), id, rect);
                out.push(ds::EditorState {
                    pane: id,
                    tab: ti,
                    path: v.path().map(|p| p.display().to_string()).unwrap_or_default(),
                    dirty: v.is_dirty(),
                    readonly: v.model.is_read_only(),
                    cursor: ds::EditorCursor { line: pos.line + 1, col: pos.col + 1 },
                    selection_chars: buf.selection().range().len(),
                    scroll_row: v.model.scroll_row(),
                    wrap_cols: v.layout.map_or(0, |l| l.cols),
                    rows: v.layout.map_or(0, |l| l.rows),
                    bar: map::editor_bar(&v.bar),
                    rect: rect(RectId::EditorBody(id)),
                    save_rect: rect(RectId::EditorSave(id)),
                    close_rect: rect(RectId::EditorClose(id)),
                    highlight: ds::EditorHighlight {
                        language: v.model.language_name(),
                        enabled: v.model.highlight_enabled(),
                        disabled_reason: v.model.highlight_off_reason(),
                        visible_classes: v.hl_stats.clone(),
                    },
                    preview: self.live_pairs.preview_of(id).and_then(|p| match self.panes.get(&p) {
                        Some(PaneView::Preview(pv)) => pv.read(cx).debug_live().map(|(provider, refreshes, banner)| ds::LivePreviewState { pane: p, provider, refreshes, banner }),
                        _ => None,
                    }),
                    preview_rect: rect(RectId::EditorPreview(id)),
                    encoding: buf.encoding().name().to_string(),
                    line_ending: buf.line_ending().name(),
                    mixed_line_endings: buf.mixed_line_endings(),
                    lossy: buf.lossy(),
                    explicit_encoding: buf.explicit_encoding().is_some(),
                    status_flash: map::editor_status_flash(v.status_flash.as_ref(), Instant::now()),
                    compare: map::editor_compare(v.compare.as_ref(), id, rect),
                    menu,
                    menu_kind,
                    bar_buttons: map::editor_bar_buttons(&v.bar, id, rect),
                    status_encoding_rect: rect(RectId::EditorEncoding(id)),
                    status_line_ending_rect: rect(RectId::EditorLineEnding(id)),
                });
            }
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn debug_tab(&self, index: usize, tab: &Tab, active: bool, tail: usize, titlebar: f32, rect: &dyn Fn(RectId) -> Option<Rect4>, cx: &App) -> ds::Tab {
        let registry = cx.global::<Agents>().registry();
        let panes = tab.tree.panes();
        // As `render_node`: a zoomed tab draws its focused pane alone.
        let multi = panes.len() > 1 && !tab.zoomed;
        ds::Tab {
            title: self.tab_bar_title(tab, cx),
            active,
            dot: marks::tab_dot(panes.iter().filter_map(|p| registry.by_pane(*p))).map(map::mark_color),
            panes: panes.iter().map(|&id| self.debug_pane(id, tab, active, multi, tail, titlebar, rect, cx)).collect(),
            rect: rect(RectId::Tab(index)),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn debug_pane(&self, id: PaneId, tab: &Tab, active: bool, multi: bool, tail: usize, titlebar: f32, rect: &dyn Fn(RectId) -> Option<Rect4>, cx: &App) -> ds::Pane {
        let session = cx.global::<Agents>().registry().by_pane(id);
        let edge = marks::edge(marks::ring_mark(session), id == tab.focused, multi);
        let view = self.panes.get(&id);
        let (kind, foreground, screen_tail, marked_text) = match view {
            Some(PaneView::Terminal(t)) => {
                let t = t.read(cx);
                let agent = &cx.global::<AppSettings>().0.agent;
                // A fresh process lookup (proc_pidinfo, argv for node / bun) on the main thread. The foreground
                // poll keeps no per-pane name to reuse (it tracks only shell / agent transitions), and this runs
                // only while answering a query, which only a gilvt started with GILVT_DEBUG_STATE=1 accepts.
                let described = t.session.foreground_pid().and_then(|pid| describe_process(pid, &agent.claude_commands, &agent.codex_commands));
                let foreground = map::foreground(described.as_ref().map(|d| d.0.as_str()), described.as_ref().and_then(|d| d.1));
                ("terminal", Some(foreground), t.session.screen_tail(tail), t.marked_text.clone())
            }
            Some(PaneView::Editor(_)) => ("editor", None, Vec::new(), None),
            Some(PaneView::Monitor(_)) => ("monitor", None, Vec::new(), None),
            Some(PaneView::Preview(_)) | None => ("preview", None, Vec::new(), None),
        };
        let (commands, running_command) = match view {
            Some(PaneView::Terminal(t)) => {
                let log = t.read(cx).commands();
                let rows = log
                    .recent(10)
                    .into_iter()
                    .map(|b| ds::CommandRow { id: b.id, running: b.running(), has_output: b.output_tail.is_some(), error_line: crate::monitor::model::error_line_of(&b), command: b.command, exit: b.exit })
                    .collect();
                (rows, log.running().map(|b| b.command.clone().unwrap_or_default()))
            }
            _ => (Vec::new(), None),
        };
        ds::Pane {
            id,
            kind,
            focused: active && id == tab.focused,
            rect: rect(RectId::Pane(id)),
            foreground,
            session: session.map(|s| map::session_id(&s.key)),
            border: map::border(edge),
            cwd: view.and_then(|v| v.cwd(cx)).map(|p| p.display().to_string()),
            screen_tail,
            marked_text,
            code: rect(RectId::PreviewCode(Some(id))),
            selection: match view {
                Some(PaneView::Preview(p)) => p.read(cx).selection_text(),
                _ => None,
            },
            header: rect(RectId::PaneHeader(id)),
            cursor: match view {
                Some(PaneView::Terminal(t)) => t.read(cx).debug_cursor().map(|(row, col, bounds)| ds::TermCursor {
                    row,
                    col,
                    rect: rects::in_frame(rects::rect_of(bounds), titlebar),
                }),
                _ => None,
            },
            commands,
            running_command,
        }
    }

    /// The sidebar row menu, else the 会话 palette's row menu.
    fn debug_menu(&self, rect: &dyn Fn(RectId) -> Option<Rect4>, cx: &App) -> Option<ds::Menu> {
        if let Some(menu) = &self.session_menu {
            let session = cx.global::<Agents>().registry().get(&menu.key);
            let (muted, ended) = (session.is_some_and(|s| s.muted), session.is_some_and(|s| !s.is_live()));
            let items = crate::sidebar::menu::items(muted, ended);
            return Some(map::menu(items.iter().map(|&(_, label, checked)| (label, checked, true)), RectId::SessionMenuItem, rect));
        }
        let items = self.sessions.as_ref()?.0.read(cx).debug_menu(cx)?;
        Some(map::menu(items, RectId::PaletteMenuItem, rect))
    }

    /// The topmost overlay (drawn last): ⌘⇧N, ⌘⇧R, ⌘P, then Quick Look.
    fn debug_overlay(&self, modifiers: Modifiers, rect: &dyn Fn(RectId) -> Option<Rect4>, cx: &App) -> Option<Overlay> {
        if let Some((panel, _)) = &self.new_agent {
            let word = self.placement(Location::from_enter(modifiers.platform, modifiers.shift), cx).word();
            return Some(panel.read(cx).debug_overlay(word, rect, cx));
        }
        if let Some((view, _)) = &self.sessions {
            return Some(view.read(cx).debug_overlay(rect, cx));
        }
        if let Some((finder, _, _)) = &self.finder {
            return Some(finder.read(cx).debug_overlay());
        }
        self.quicklook.as_ref().map(|(ql, _)| ql.read(cx).debug_overlay(rect))
    }

    fn debug_inspector(&self, rect: &dyn Fn(RectId) -> Option<Rect4>, cx: &App) -> ds::Inspector {
        let hidden = ds::Inspector {
            visible: false,
            tab: "process",
            card: None,
            banner: None,
            timeline_rows: 0,
            banner_rect: None,
            width: self.inspector.width,
            filter: self.inspector.timeline.filter.label(),
            chips: Vec::new(),
            toast: None,
            rows: Vec::new(),
            artifacts: None,
            config: None,
        };
        if self.inspector.hidden {
            return hidden;
        }
        let agents = cx.global::<Agents>();
        let registry = agents.registry();
        let session = self.focused_pane().and_then(|p| crate::inspector::model::follow(registry.sessions(), p));
        let reachable = |p: PaneId| self.has_pane(p) || workspaces(cx).iter().any(|w| w.read(cx).is_ok_and(|o| o.has_pane(p)));
        let banner = crate::inspector::model::banner(registry.sessions(), session, reachable, |s| agents.project(s), Instant::now());
        let entries = session.map_or(&[][..], |s| self.inspector.timeline.entries_for(&s.key));
        ds::Inspector {
            visible: true,
            card: session.map(|s| ds::Card {
                status: map::status_name(&s.status),
                turn: agents.timeline(&s.key).and_then(|v| v.current().map(|t| t.index)),
            }),
            banner_rect: banner.as_ref().and(rect(RectId::InspectorBanner)),
            banner: banner.map(|b| map::banner_text(&b.title)),
            timeline_rows: session.map_or(0, |s| self.inspector.timeline.rows_for(&s.key)),
            chips: timeline::chips(entries, rect),
            rows: timeline::rows(entries, rect),
            toast: self.inspector.note.map(|(note, _)| note.to_string()),
            tab: self.inspector.tab.name(),
            artifacts: self.debug_artifacts(session, rect, cx),
            config: self.debug_config(session, rect),
            ..hidden
        }
    }

    fn debug_config(&self, session: Option<&gilvt_agent::Session>, rect: &dyn Fn(RectId) -> Option<Rect4>) -> Option<ds::ConfigSummary> {
        if self.inspector.tab != crate::inspector::InspectorTab::Config {
            return None;
        }
        let empty = session.is_none().then_some(if self.focused_pane().is_some() { "shell" } else { "no_session" });
        let summary = self.inspector.config.summary.as_deref();
        let layer = |source: gilvt_config::SourceLayer| match source {
            gilvt_config::SourceLayer::Runtime => "runtime",
            gilvt_config::SourceLayer::User => "user",
            gilvt_config::SourceLayer::Project => "project",
            gilvt_config::SourceLayer::Local => "local",
        };
        let value = |value: &gilvt_config::Value| ds::ConfigValue { text: value.text.clone(), source: layer(value.source) };
        let disclosure = &self.inspector.config.disclosure;
        // Expandable rows are numbered in drawing order: MCP, then hooks, then memory (the 「扩展」 card between
        // MCP and hooks has none), matching `config_view::cards`.
        let mut row = 0;
        let mut items = |section: crate::inspector::config_model::Section, items: &[gilvt_config::NamedItem]| -> Vec<ds::ConfigItem> {
            items
                .iter()
                .map(|item| {
                    let ix = row;
                    row += 1;
                    let expandable = !item.lines.is_empty() || item.path.is_some();
                    let expanded = expandable && disclosure.is_open(section, &item.name);
                    ds::ConfigItem {
                        name: item.name.clone(),
                        detail: item.detail.clone(),
                        source: layer(item.source),
                        lines: item.lines.clone(),
                        path: item.path.as_ref().map(|p| p.display().to_string()),
                        expanded,
                        row_rect: rect(RectId::ConfigRow(ix)),
                        open_rect: if expanded { rect(RectId::ConfigOpen(ix)) } else { None },
                    }
                })
                .collect()
        };
        use crate::inspector::config_model::{Group, Section};
        let mcp = summary.map_or_else(Vec::new, |s| items(Section::Mcp, &s.mcp));
        let hooks = summary.map_or_else(Vec::new, |s| items(Section::Hooks, &s.hooks));
        let memory = summary.map_or_else(Vec::new, |s| items(Section::Memory, &s.memory));
        let dialog = summary.zip(disclosure.dialog).map(|(summary, group)| ds::ConfigDialog {
            group: group.name(),
            title: group.title(summary.agent).to_string(),
            rows: group
                .items(summary)
                .iter()
                .enumerate()
                .map(|(ix, r)| ds::ConfigDialogRow {
                    name: r.name.clone(),
                    description: r.description.clone(),
                    source: layer(r.source),
                    path: r.path.display().to_string(),
                    rect: rect(RectId::ConfigDialogRow(ix)),
                    edit_rect: rect(RectId::ConfigDialogEdit(ix)),
                })
                .collect(),
            close_rect: rect(RectId::ConfigDialogClose),
        });
        Some(ds::ConfigSummary {
            loading: self.inspector.config.loading,
            empty,
            agent: summary.map(|summary| summary.agent.name()),
            cwd: summary.map(|summary| summary.cwd.display().to_string()),
            model: summary.and_then(|summary| summary.model.as_ref()).map(value),
            permission: summary.and_then(|summary| summary.permission.as_ref()).map(value),
            mcp,
            hooks,
            skills: summary.map_or(0, |summary| Group::Skills.items(summary).len()),
            commands: summary.map_or(0, |summary| Group::Commands.items(summary).len()),
            subagents: summary.map_or(0, |summary| Group::Subagents.items(summary).len()),
            group_rects: (0..3).map(|ix| rect(RectId::ConfigGroup(ix))).collect(),
            expanded: disclosure.open_rows(),
            dialog,
            memory,
            sources: summary.map_or_else(Vec::new, |summary| {
                summary
                    .sources
                    .iter()
                    .map(|source| ds::ConfigSource { path: source.path.display().to_string(), source: layer(source.source) })
                    .collect()
            }),
            warnings: summary.map_or_else(Vec::new, |summary| summary.warnings.clone()),
            refresh_rect: rect(RectId::InspectorConfigRefresh),
        })
    }

    /// The 「产物」 tab as drawn (None unless it is the current tab). Reads the tab's UI state without
    /// switching it (`&self`): another session's selection / open cards are reported as empty.
    fn debug_artifacts(&self, session: Option<&gilvt_agent::Session>, rect: &dyn Fn(RectId) -> Option<Rect4>, cx: &App) -> Option<ds::Artifacts> {
        use crate::inspector::artifacts_model as am;
        if self.inspector.tab != crate::inspector::InspectorTab::Artifacts {
            return None;
        }
        let Some(session) = session else {
            let empty = if self.focused_pane().is_some() { "shell" } else { "no_session" };
            return Some(ds::Artifacts { summary: None, net: None, quiet_groups: Vec::new(), empty: Some(empty), cards: Vec::new() });
        };
        let built = super::inspector::build_artifacts(cx.global::<Agents>(), session);
        let ui = &self.inspector.artifacts;
        let mine = ui.session.as_ref() == Some(&session.key);
        let newest = am::newest_open_by_default(&built);
        let file = |f: &am::FileRow, sel: am::Sel, rect: Option<Rect4>| ds::ArtifactFile {
            path: f.path.clone(),
            status: match f.status {
                'A' => "A",
                'D' => "D",
                'R' => "R",
                _ => "M",
            },
            added: f.added,
            removed: f.removed,
            binary: f.binary,
            selected: mine && ui.selected == Some(sel),
            rect,
        };
        let net = built.net.as_ref().map(|n| {
            let open = mine && ui.net_open;
            ds::ArtifactsNet {
                open,
                selected: mine && ui.selected == Some(am::Sel::Net),
                computing: n.computing,
                range_label: n.range_label.clone(),
                // The view draws no rows for a failed net.
                files: if open && n.failed.is_none() {
                    n.files.iter().enumerate().map(|(i, f)| file(f, am::Sel::NetFile(i), rect(RectId::ArtifactNetFile(i)))).collect()
                } else {
                    Vec::new()
                },
                excluded: n.excluded,
                only_repo: n.only_repo.clone(),
                failed: n.failed.clone(),
                rect: rect(RectId::ArtifactNet),
            }
        });
        let quiet_groups = built
            .quiet_groups
            .iter()
            .enumerate()
            .map(|(gi, g)| ds::ArtifactQuietGroup {
                key: g.key,
                count: g.cards.len(),
                titles: g.titles.clone(),
                open: mine && ui.open_quiet.contains(&g.key),
                selected: mine && ui.selected == Some(am::Sel::Quiet(g.key)),
                rect: rect(RectId::ArtifactQuiet(gi)),
            })
            .collect::<Vec<_>>();
        let cards = built
            .cards
            .iter()
            .enumerate()
            .map(|(ci, c)| {
                let all = mine && ui.all_files.contains(&c.key);
                let shown = if all { c.files.len() } else { c.files.len().min(am::SHOWN_FILES) };
                let expanded = if mine { ui.is_open(c.key, Some(c.key) == newest) } else { Some(c.key) == newest };
                let selected = |sel| mine && ui.selected == Some(sel);
                let drawn = c.quiet_group.is_none_or(|g| mine && ui.open_quiet.contains(&g));
                ds::ArtifactCard {
                    turn: c.turn,
                    turns: c.turns.clone(),
                    follow_ups: c.follow_ups.clone(),
                    started: c.started.clone(),
                    computing: c.computing,
                    counts_hidden: c.counts_hidden,
                    quiet_group: c.quiet_group,
                    touched_later_turn: c.touched_later_turn,
                    sub_line: crate::inspector::artifacts_view::sub_line(c),
                    title: c.title.clone(),
                    state: c.state.name(),
                    notices: c.notices.iter().map(|n| n.text()).collect(),
                    expanded,
                    selected: selected(am::Sel::Card(c.key)),
                    touched_later: c.touched_later,
                    test: c.test.as_ref().map(|t| ds::ArtifactTest { ok: t.ok, command: t.command.clone(), exit: t.exit }),
                    quote: c.quote.clone(),
                    hidden_files: c.files.len() - shown,
                    // As drawn: nothing for a collapsed or quiet card, or one folded into a closed quiet group.
                    files: if drawn && expanded && c.state != am::CardState::Quiet {
                        c.files.iter().enumerate().take(shown).map(|(fi, f)| file(f, am::Sel::File(c.key, fi), rect(RectId::ArtifactFile(ci, fi)))).collect()
                    } else {
                        Vec::new()
                    },
                    rect: rect(RectId::ArtifactCard(ci)),
                    more_rect: rect(RectId::ArtifactMore(ci)),
                }
            })
            .collect::<Vec<_>>();
        let empty = cards.is_empty().then_some("no_turns");
        let summary = built.summary.map(|s| ds::ArtifactsSummary { files: s.files, added: s.added, removed: s.removed, computing: s.computing });
        Some(ds::Artifacts { summary, net, quiet_groups, empty, cards })
    }
}
