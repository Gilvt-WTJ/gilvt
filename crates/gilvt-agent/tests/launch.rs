//! Launch / resume command lines (spec §2.1, §3.4).

use std::path::{Path, PathBuf};

use gilvt_agent::{
    new_agent_command, resume_command, shell_quote, AgentKind, ClaudePermission, CodexPermission, NewAgent, Permission,
};

fn agent(agent: AgentKind, dir: &str) -> NewAgent {
    NewAgent { agent, dir: PathBuf::from(dir), model: None, permission: None, prompt: None }
}

#[test]
fn quotes_like_the_hook_installer() {
    assert_eq!(shell_quote("claude"), "claude");
    assert_eq!(shell_quote("/Users/u/src/gilvt-lab"), "/Users/u/src/gilvt-lab");
    assert_eq!(shell_quote("claude-sonnet-4.5@x+y%z,w=v:1"), "claude-sonnet-4.5@x+y%z,w=v:1");
    assert_eq!(shell_quote(""), "''");
    assert_eq!(shell_quote("/Users/u/My Project"), "'/Users/u/My Project'");
    assert_eq!(shell_quote("it's"), r"'it'\''s'");
    assert_eq!(shell_quote(r#"say "hi""#), r#"'say "hi"'"#);
    assert_eq!(shell_quote("/Users/u/项目"), "'/Users/u/项目'");
    assert_eq!(shell_quote("a\nb"), "'a\nb'");
    assert_eq!(shell_quote("$HOME `x` \\ !"), "'$HOME `x` \\ !'");
    assert_eq!(shell_quote("~/src"), "'~/src'");
    assert_eq!(shell_quote("*.rs;rm"), "'*.rs;rm'");
}

#[test]
fn bare_new_agents_just_cd_and_launch() {
    assert_eq!(
        new_agent_command("claude", &agent(AgentKind::Claude, "/Users/u/src/app")),
        "cd /Users/u/src/app && claude"
    );
    assert_eq!(
        new_agent_command("codex-w", &agent(AgentKind::Codex, "/Users/u/My App")),
        "cd '/Users/u/My App' && codex-w"
    );
}

#[test]
fn claude_flags() {
    let mut a = agent(AgentKind::Claude, "/p");
    a.model = Some("sonnet".into());
    assert_eq!(new_agent_command("claude", &a), "cd /p && claude --model sonnet");
    a.permission = Some(Permission::Claude(ClaudePermission::AcceptEdits));
    assert_eq!(new_agent_command("claude", &a), "cd /p && claude --model sonnet --permission-mode acceptEdits");
    a.prompt = Some("修一下 README 里的错别字".into());
    assert_eq!(
        new_agent_command("claude", &a),
        "cd /p && claude --model sonnet --permission-mode acceptEdits '修一下 README 里的错别字'"
    );
    a.model = None;
    assert_eq!(new_agent_command("claude", &a), "cd /p && claude --permission-mode acceptEdits '修一下 README 里的错别字'");
    a.permission = None;
    assert_eq!(new_agent_command("claude", &a), "cd /p && claude '修一下 README 里的错别字'");
    a.model = Some("claude-opus-4-1[1m]".into());
    assert_eq!(new_agent_command("claude", &a), "cd /p && claude --model 'claude-opus-4-1[1m]' '修一下 README 里的错别字'");
}

#[test]
fn every_claude_permission_mode() {
    let modes: Vec<String> = ClaudePermission::ALL
        .iter()
        .map(|&p| {
            let mut a = agent(AgentKind::Claude, "/p");
            a.permission = Some(Permission::Claude(p));
            new_agent_command("claude", &a)
        })
        .collect();
    assert_eq!(
        modes,
        [
            "cd /p && claude --permission-mode manual",
            "cd /p && claude --permission-mode acceptEdits",
            "cd /p && claude --permission-mode plan",
            "cd /p && claude --permission-mode auto",
            "cd /p && claude --permission-mode dontAsk",
            "cd /p && claude --permission-mode bypassPermissions",
        ]
    );
    let labels: Vec<&str> = ClaudePermission::ALL.iter().map(|p| p.label()).collect();
    assert_eq!(labels, ["manual", "acceptEdits", "plan", "auto", "dontAsk", "bypassPermissions"]);
}

#[test]
fn codex_flags() {
    let mut a = agent(AgentKind::Codex, "/p");
    a.model = Some("gpt-5.5".into());
    assert_eq!(new_agent_command("codex", &a), "cd /p && codex -m gpt-5.5");
    a.prompt = Some("看看测试为什么挂".into());
    assert_eq!(new_agent_command("codex", &a), "cd /p && codex -m gpt-5.5 '看看测试为什么挂'");
    a.model = None;
    a.permission = Some(Permission::Codex(CodexPermission::Auto));
    assert_eq!(
        new_agent_command("codex", &a),
        "cd /p && codex -s workspace-write -a on-request '看看测试为什么挂'"
    );
}

#[test]
fn every_codex_permission_preset() {
    let cmds: Vec<String> = CodexPermission::ALL
        .iter()
        .map(|&p| {
            let mut a = agent(AgentKind::Codex, "/p");
            a.model = Some("gpt-5.5".into());
            a.permission = Some(Permission::Codex(p));
            new_agent_command("codex-w", &a)
        })
        .collect();
    assert_eq!(
        cmds,
        [
            "cd /p && codex-w -m gpt-5.5 -s read-only -a on-request",
            "cd /p && codex-w -m gpt-5.5 -s workspace-write -a on-request",
            "cd /p && codex-w -m gpt-5.5 -s danger-full-access -a never",
        ]
    );
    let labels: Vec<&str> = CodexPermission::ALL.iter().map(|p| p.label()).collect();
    assert_eq!(labels, ["只读", "自动", "完全访问"]);
}

#[test]
fn mismatched_permission_is_ignored() {
    let mut a = agent(AgentKind::Claude, "/p");
    a.permission = Some(Permission::Codex(CodexPermission::FullAccess));
    assert_eq!(new_agent_command("claude", &a), "cd /p && claude");
    let mut a = agent(AgentKind::Codex, "/p");
    a.permission = Some(Permission::Claude(ClaudePermission::BypassPermissions));
    assert_eq!(new_agent_command("codex", &a), "cd /p && codex");
}

#[test]
fn blank_model_and_prompt_are_omitted() {
    let mut a = agent(AgentKind::Claude, "/p");
    a.model = Some("  ".into());
    a.prompt = Some(" \n\t ".into());
    assert_eq!(new_agent_command("claude", &a), "cd /p && claude");
    a.model = Some(" opus ".into());
    a.prompt = Some("\n  fix it  \n".into());
    assert_eq!(new_agent_command("claude", &a), "cd /p && claude --model opus 'fix it'");
}

#[test]
fn prompts_are_one_quoted_argument() {
    let mut a = agent(AgentKind::Codex, "/Users/u/o'neil");
    a.prompt = Some("第一行\n  it's $HOME and \"quotes\"\n第三行".into());
    assert_eq!(
        new_agent_command("codex", &a),
        "cd '/Users/u/o'\\''neil' && codex '第一行\n  it'\\''s $HOME and \"quotes\"\n第三行'"
    );
    // A single safe word needs no quotes.
    a.prompt = Some("continue".into());
    assert_eq!(new_agent_command("codex", &a), "cd '/Users/u/o'\\''neil' && codex continue");
}

#[test]
fn prompts_that_look_like_flags_follow_a_double_dash() {
    let mut a = agent(AgentKind::Claude, "/p");
    a.prompt = Some("-p 是什么意思".into());
    assert_eq!(new_agent_command("claude", &a), "cd /p && claude -- '-p 是什么意思'");
    let mut a = agent(AgentKind::Codex, "/p");
    a.prompt = Some("--help".into());
    assert_eq!(new_agent_command("codex", &a), "cd /p && codex -- --help");
}

#[test]
fn resume_commands() {
    assert_eq!(
        resume_command(
            "claude",
            AgentKind::Claude,
            "0b7e5c3a-1d2f-4e5a-8b9c-0d1e2f3a4b5c",
            Path::new("/Users/u/gilvt-lab")
        ),
        "cd /Users/u/gilvt-lab && claude --resume 0b7e5c3a-1d2f-4e5a-8b9c-0d1e2f3a4b5c"
    );
    assert_eq!(
        resume_command(
            "codex-w",
            AgentKind::Codex,
            "01a0eb29-4056-78f2-a118-864580a5a3fb",
            Path::new("/Users/u/My Lab")
        ),
        "cd '/Users/u/My Lab' && codex-w resume 01a0eb29-4056-78f2-a118-864580a5a3fb"
    );
    // Ids come from files on disk: quote them like any other argument.
    assert_eq!(resume_command("claude", AgentKind::Claude, "a b", Path::new("/p")), "cd /p && claude --resume 'a b'");
}

#[test]
fn permissions_round_trip_through_json() {
    let claude = ClaudePermission::ALL.map(Permission::Claude);
    let codex = CodexPermission::ALL.map(Permission::Codex);
    for p in claude.into_iter().chain(codex) {
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(serde_json::from_str::<Permission>(&json).unwrap(), p, "{json}");
    }
    assert_eq!(Permission::Claude(ClaudePermission::Plan).agent(), AgentKind::Claude);
    assert_eq!(Permission::Codex(CodexPermission::ReadOnly).agent(), AgentKind::Codex);
    assert_eq!(Permission::Codex(CodexPermission::ReadOnly).label(), "只读");
}
