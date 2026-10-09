//! Which `ssh` invocations `gilvt ssh` takes over (spec §3.1): an interactive login, or `-t` with a
//! remote command. Everything else is passed to ssh untouched.

/// Options that take an argument (OpenSSH 9/10 `ssh -h`).
const WITH_ARG: &str = "BbcDEeFIiJLlmOoPpQRSWw";
/// Flags that mean "not an interactive login we should touch".
const PASSTHROUGH: &str = "NfWOTGVQsMS";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshArgs {
    pub opts: Vec<String>,
    pub destination: String,
    pub command: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Parsed { Passthrough, Login(SshArgs) }

/// `-o Key=value` options that change session/multiplexing behaviour: not ours to touch.
fn o_forces_passthrough(val: &str) -> bool {
    let key = val.trim_start().split(|c: char| c == '=' || c.is_whitespace()).next().unwrap_or("").to_ascii_lowercase();
    ["sessiontype", "forkafterauthentication", "controlmaster", "controlpath", "controlpersist", "requesttty", "remotecommand", "stdinnull"].contains(&key.as_str())
}

pub fn parse(argv: &[String]) -> Parsed {
    let mut opts = Vec::new();
    let mut tty = false;
    let mut i = 0;
    while i < argv.len() {
        let a = &argv[i];
        if a == "--" { i += 1; break; }
        if !a.starts_with('-') || a == "-" { break; }
        let flags: Vec<char> = a[1..].chars().collect();
        let mut j = 0;
        let mut takes_next = false;
        while j < flags.len() {
            let f = flags[j];
            if PASSTHROUGH.contains(f) { return Parsed::Passthrough; }
            if f == 't' { tty = true; }
            if WITH_ARG.contains(f) {
                if f == 'o' {
                    let val: String = if j + 1 == flags.len() { argv.get(i + 1).cloned().unwrap_or_default() } else { flags[j + 1..].iter().collect() };
                    if o_forces_passthrough(&val) { return Parsed::Passthrough; }
                }
                if j + 1 == flags.len() { takes_next = true; }
                break; // the rest of this word is the argument
            }
            j += 1;
        }
        opts.push(a.clone());
        if takes_next {
            let Some(v) = argv.get(i + 1) else { return Parsed::Passthrough };
            opts.push(v.clone());
            i += 1;
        }
        i += 1;
    }
    let Some(destination) = argv.get(i).cloned() else { return Parsed::Passthrough };
    let mut rest = &argv[i + 1..];
    // OpenSSH keeps parsing options after the destination: `ssh -t host -l root`.
    if rest.first().is_some_and(|a| a.starts_with('-') && a != "--") { return Parsed::Passthrough; }
    if rest.first().is_some_and(|a| a == "--") { rest = &rest[1..]; }
    let command = rest.to_vec();
    if !command.is_empty() && !tty { return Parsed::Passthrough; }
    Parsed::Login(SshArgs { opts, destination, command })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn v(s: &[&str]) -> Vec<String> { s.iter().map(|x| x.to_string()).collect() }
    fn login(a: &[&str]) -> SshArgs { match parse(&v(a)) { Parsed::Login(s) => s, Parsed::Passthrough => panic!("passthrough: {a:?}") } }

    #[test]
    fn plain_login() {
        let s = login(&["devbox"]);
        assert_eq!((s.opts.len(), s.destination.as_str(), s.command.len()), (0, "devbox", 0));
        let s = login(&["-p", "2222", "-i", "~/.ssh/k", "-o", "User=dev", "-J", "jump", "-A", "dev@devbox"]);
        assert_eq!(s.opts, v(&["-p", "2222", "-i", "~/.ssh/k", "-o", "User=dev", "-J", "jump", "-A"]));
        assert_eq!(s.destination, "dev@devbox");
        let s = login(&["-p2222", "devbox"]);
        assert_eq!(s.opts, v(&["-p2222"]));
    }

    #[test]
    fn tty_with_command_is_a_login() {
        let s = login(&["-t", "devbox", "tmux", "a"]);
        assert_eq!(s.command, v(&["tmux", "a"]));
        let s = login(&["-tt", "devbox", "--", "tmux a"]);
        assert_eq!(s.command, v(&["tmux a"]));
        let s = login(&["-At", "devbox", "tmux"]);
        assert_eq!(s.command, v(&["tmux"]));
    }

    #[test]
    fn options_after_destination_pass_through() {
        for a in [&["-t", "devbox", "-l", "root"][..], &["-t", "devbox", "-N"]] {
            assert!(matches!(parse(&v(a)), Parsed::Passthrough), "{a:?}");
        }
        assert_eq!(login(&["-t", "devbox", "--", "tmux a"]).command, v(&["tmux a"]));
    }

    #[test]
    fn o_forms_of_passthrough_options() {
        for a in [&["-o", "SessionType=none", "devbox"][..], &["-oRemoteCommand=x", "devbox"], &["-o", "RequestTTY no", "devbox"],
                  &["-o", "controlmaster=auto", "devbox"], &["-oStdinNull=yes", "devbox"], &["-o", "ControlPath=/x", "devbox"]] {
            assert!(matches!(parse(&v(a)), Parsed::Passthrough), "{a:?}");
        }
        login(&["-o", "User=dev", "-oServerAliveInterval=5", "devbox"]);
    }

    #[test]
    fn everything_else_passes_through() {
        for a in [&["devbox", "uname"][..], &["-N", "devbox"], &["-f", "-N", "devbox"], &["-W", "h:22", "jump"], &["-O", "check", "devbox"],
                  &["-T", "devbox"], &["-G", "devbox"], &["-V"], &["-Q", "cipher"], &["-s", "devbox", "sftp"], &["-M", "devbox"],
                  &["-S", "/tmp/x", "devbox"], &[], &["-p"], &["-fN", "devbox"]] {
            assert!(matches!(parse(&v(a)), Parsed::Passthrough), "{a:?}");
        }
    }
}
