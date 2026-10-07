use std::sync::Arc;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::term::ClipboardType;

use crate::osc::Notification;
use crate::shellmarks::{PromptMark, ReportedCwd};
use crate::palette::Rgb;

pub type ColorFormatter = Arc<dyn Fn(Rgb) -> String + Send + Sync>;
pub type SizeFormatter = Arc<dyn Fn(WindowSize) -> String + Send + Sync>;

/// Events from a terminal session, delivered to the UI thread over a channel.
#[derive(Clone)]
pub enum TermEvent {
    /// New content is available; repaint.
    Wakeup,
    Title(String),
    ResetTitle,
    Bell,
    /// OSC 52 copy request.
    ClipboardStore(String),
    /// Reply that must be written back to the PTY (DSR, DA, ...).
    PtyWrite(String),
    ColorRequest(usize, ColorFormatter),
    TextAreaSizeRequest(SizeFormatter),
    /// OSC 9 / OSC 777 desktop notification.
    Notification(Notification),
    /// OSC 7 working directory reported by shell integration.
    Cwd(ReportedCwd),
    /// OSC 133 prompt / command mark.
    Prompt(PromptMark),
    /// The command line of the OSC 133;C mark that follows (`cmdline_url`), sent just before its `Prompt`.
    CommandLine(String),
    /// The output of the command whose OSC 133;D follows (sent just before that `Prompt`): the PTY bytes since
    /// its C as text (`capture::render`); None when no command was running or it used the alternate screen.
    CommandOutput(Option<String>),
    /// The child process exited (with exit code when known).
    ChildExit(Option<i32>),
}

impl std::fmt::Debug for TermEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TermEvent::Wakeup => write!(f, "Wakeup"),
            TermEvent::Title(t) => write!(f, "Title({t})"),
            TermEvent::ResetTitle => write!(f, "ResetTitle"),
            TermEvent::Bell => write!(f, "Bell"),
            TermEvent::ClipboardStore(s) => write!(f, "ClipboardStore({s})"),
            TermEvent::PtyWrite(s) => write!(f, "PtyWrite({s:?})"),
            TermEvent::ColorRequest(i, _) => write!(f, "ColorRequest({i})"),
            TermEvent::TextAreaSizeRequest(_) => write!(f, "TextAreaSizeRequest"),
            TermEvent::Notification(n) => write!(f, "Notification({n:?})"),
            TermEvent::Cwd(c) => write!(f, "Cwd({c:?})"),
            TermEvent::Prompt(m) => write!(f, "Prompt({m:?})"),
            TermEvent::CommandLine(c) => write!(f, "CommandLine({c:?})"),
            TermEvent::CommandOutput(o) => write!(f, "CommandOutput({o:?})"),
            TermEvent::ChildExit(c) => write!(f, "ChildExit({c:?})"),
        }
    }
}

/// Bridges alacritty events into a `TermEvent` channel.
#[derive(Clone)]
pub struct EventProxy(pub async_channel::Sender<TermEvent>);

impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        let mapped = match event {
            Event::Wakeup => TermEvent::Wakeup,
            Event::Title(t) => TermEvent::Title(t),
            Event::ResetTitle => TermEvent::ResetTitle,
            Event::Bell => TermEvent::Bell,
            Event::ClipboardStore(ClipboardType::Clipboard, text) => TermEvent::ClipboardStore(text),
            Event::PtyWrite(text) => TermEvent::PtyWrite(text),
            Event::ColorRequest(i, f) => TermEvent::ColorRequest(i, f),
            Event::TextAreaSizeRequest(f) => TermEvent::TextAreaSizeRequest(f),
            Event::ChildExit(status) => TermEvent::ChildExit(status.code()),
            // Clipboard reads (OSC 52 paste) are denied; selection clipboard does not exist on macOS.
            Event::ClipboardStore(..)
            | Event::ClipboardLoad(..)
            | Event::CursorBlinkingChange
            | Event::MouseCursorDirty
            | Event::Exit => return,
        };
        let _ = self.0.try_send(mapped);
    }
}
