//! `gilvt ssh` links on the app side.

/// Placeholder until the link manager lands: answers every `RemoteBegin` with an error.
pub fn answer_begin(q: gilvt_ipc::Query, _cx: &mut gpui::App) {
    q.respond(gilvt_ipc::Response::Error { message: "not yet".into() })
}
