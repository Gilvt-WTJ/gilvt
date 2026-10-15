//! Finding an agent below a wrapper in a pane's foreground (agent-shims spec §4.1). Pure: the process table comes in.

use gilvt_agent::{AgentKind, ProcRow};

/// How many levels below the foreground leader are searched.
pub const MAX_DEPTH: usize = 4;

/// The shallowest process below `leader`, in `leader`'s process group, that `classify` calls an agent.
pub fn find_agent(table: &[ProcRow], leader: u32, mut classify: impl FnMut(&ProcRow) -> Option<AgentKind>) -> Option<(u32, AgentKind)> {
    let pgid = table.iter().find(|r| r.pid == leader)?.pgid;
    let mut level = vec![leader];
    for _ in 0..MAX_DEPTH {
        let next: Vec<&ProcRow> = table.iter().filter(|r| r.pgid == pgid && level.contains(&r.ppid)).collect();
        if let Some((r, kind)) = next.iter().find_map(|r| classify(r).map(|k| (*r, k))) {
            return Some((r.pid, kind));
        }
        level = next.iter().map(|r| r.pid).collect();
        if level.is_empty() {
            break;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(pid: u32, ppid: u32, pgid: u32, name: &str) -> ProcRow {
        ProcRow { pid, ppid, pgid, name: name.into() }
    }

    fn claude_named(r: &ProcRow) -> Option<AgentKind> {
        (r.name == "claude").then_some(AgentKind::Claude)
    }

    #[test]
    fn finds_an_agent_child_of_a_wrapper() {
        let t = [row(10, 1, 10, "node"), row(11, 10, 10, "claude")];
        assert_eq!(find_agent(&t, 10, claude_named), Some((11, AgentKind::Claude)));
    }

    #[test]
    fn looks_four_levels_down_and_no_further() {
        let t = [row(10, 1, 10, "node"), row(11, 10, 10, "sh"), row(12, 11, 10, "sh"), row(13, 12, 10, "sh"), row(14, 13, 10, "claude")];
        assert_eq!(find_agent(&t, 10, claude_named), Some((14, AgentKind::Claude)));
        let deeper = [&t[..4], &[row(14, 13, 10, "sh"), row(15, 14, 10, "claude")]].concat();
        assert_eq!(find_agent(&deeper, 10, claude_named), None);
    }

    #[test]
    fn ignores_children_in_another_process_group() {
        let t = [row(10, 1, 10, "node"), row(11, 10, 99, "claude")];
        assert_eq!(find_agent(&t, 10, claude_named), None, "a background job of the wrapper");
    }

    #[test]
    fn breadth_first_picks_the_shallowest() {
        let t = [row(10, 1, 10, "node"), row(11, 10, 10, "sh"), row(12, 11, 10, "claude"), row(13, 10, 10, "claude")];
        assert_eq!(find_agent(&t, 10, claude_named), Some((13, AgentKind::Claude)));
    }
}
