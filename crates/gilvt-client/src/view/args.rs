//! Command-line parsing for `gilvt view` / `gilvt diff` (hand-written: two subcommands, a few flags).

use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Stdin,
    Path(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    View { target: Target, pin: bool, as_type: Option<String> },
    Diff { rev: Option<String> },
    Help,
}

pub fn parse(args: &[String]) -> Result<Command, String> {
    let mut it = args.iter();
    match it.next().map(String::as_str) {
        None | Some("-h") | Some("--help") | Some("help") => Ok(Command::Help),
        Some("view") => {
            let (mut pin, mut as_type, mut target) = (false, None, None);
            while let Some(a) = it.next() {
                match a.as_str() {
                    "-h" | "--help" => return Ok(Command::Help),
                    "--pin" => pin = true,
                    "--as" => as_type = Some(it.next().ok_or("--as needs a type, e.g. --as md")?.clone()),
                    "-" => target = Some(Target::Stdin),
                    s if s.starts_with("--") => return Err(format!("unknown option {s}")),
                    s => {
                        if target.is_some() {
                            return Err("gilvt view takes one file".into());
                        }
                        target = Some(Target::Path(s.to_string()));
                    }
                }
            }
            let target = target.ok_or("gilvt view needs a file, or - for stdin")?;
            Ok(Command::View { target, pin, as_type })
        }
        Some("diff") => {
            let rev = it.next().cloned();
            if matches!(rev.as_deref(), Some("-h" | "--help")) {
                return Ok(Command::Help);
            }
            if it.next().is_some() {
                return Err("gilvt diff takes at most one revision".into());
            }
            Ok(Command::Diff { rev })
        }
        Some(other) => Err(format!("unknown command {other}")),
    }
}

/// Splits `file:line[:col]`, unless the whole string is itself an existing path.
pub fn split_location(raw: &str, exists: impl Fn(&Path) -> bool) -> (String, Option<u32>, Option<u32>) {
    if exists(Path::new(raw)) {
        return (raw.to_string(), None, None);
    }
    let mut parts = raw.rsplitn(3, ':');
    let last = parts.next().unwrap_or_default();
    let mid = parts.next();
    let first = parts.next();
    match (first, mid.and_then(|m| m.parse().ok()), last.parse().ok()) {
        (Some(path), Some(line), Some(col)) => (path.to_string(), Some(line), Some(col)),
        _ => match (mid, last.parse().ok()) {
            (Some(_), Some(line)) => {
                let path = &raw[..raw.len() - last.len() - 1];
                (path.to_string(), Some(line), None)
            }
            _ => (raw.to_string(), None, None),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parses_view_and_diff() {
        assert_eq!(parse(&s(&["view", "a.rs"])).unwrap(), Command::View { target: Target::Path("a.rs".into()), pin: false, as_type: None });
        assert_eq!(
            parse(&s(&["view", "--pin", "--as", "md", "-"])).unwrap(),
            Command::View { target: Target::Stdin, pin: true, as_type: Some("md".into()) }
        );
        assert_eq!(parse(&s(&["diff"])).unwrap(), Command::Diff { rev: None });
        assert_eq!(parse(&s(&["diff", "main"])).unwrap(), Command::Diff { rev: Some("main".into()) });
        assert_eq!(parse(&s(&[])).unwrap(), Command::Help);
    }

    #[test]
    fn help_after_a_subcommand_is_help() {
        assert_eq!(parse(&s(&["view", "--help"])).unwrap(), Command::Help);
        assert_eq!(parse(&s(&["view", "a.rs", "-h"])).unwrap(), Command::Help);
        assert_eq!(parse(&s(&["diff", "--help"])).unwrap(), Command::Help);
    }

    #[test]
    fn rejects_bad_usage() {
        assert!(parse(&s(&["view"])).is_err());
        assert!(parse(&s(&["view", "a", "b"])).is_err());
        assert!(parse(&s(&["view", "--as"])).is_err());
        assert!(parse(&s(&["view", "--nope", "a"])).is_err());
        assert!(parse(&s(&["diff", "a", "b"])).is_err());
        assert!(parse(&s(&["frob"])).is_err());
    }

    #[test]
    fn splits_locations() {
        let none = |_: &Path| false;
        assert_eq!(split_location("a.rs:12:5", none), ("a.rs".into(), Some(12), Some(5)));
        assert_eq!(split_location("a.rs:12", none), ("a.rs".into(), Some(12), None));
        assert_eq!(split_location("a.rs", none), ("a.rs".into(), None, None));
        assert_eq!(split_location("dir:x/a.rs", none), ("dir:x/a.rs".into(), None, None));
        // A file literally named "notes:12" wins over the line-number reading.
        assert_eq!(split_location("notes:12", |p: &Path| p == Path::new("notes:12")), ("notes:12".into(), None, None));
    }
}
