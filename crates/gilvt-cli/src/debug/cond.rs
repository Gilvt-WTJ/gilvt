//! The condition language of `gilvt debug wait` (deliberately small, not JSONPath).
//!
//! - A condition is clauses joined by `&&`; it holds when every clause does.
//! - Clause: `<path> <op> <value> [count=N]`, `<path> exists [count=N]` or `<path> !exists`.
//! - Path: keys (`a.b`, letters / digits / `_` / `-`, any script), `[N]` (index), `[*]` (every
//!   element of an array), `[?field==<value>]` (array elements whose `field` equals the value) and
//!   `[?field contains <value>]` (whose `field` contains it, as the `contains` op), chained (a key
//!   follows the start or a `.`; blanks are allowed before `]`): `windows[0].sidebar.rows[?name=="新会话"].status`. A path may start with a `[…]`.
//! - Op: `==`, `!=`, `contains` (substring of a string, or an element of an array). Values are JSON
//!   literals; numbers compare by value (`1 == 1.0`).
//!
//! A path yields candidates: none where a key or index is missing, several through `[*]` / `[?]`.
//! Without `count`, `==` / `contains` hold if any candidate matches, and `!=` holds if there is at
//! least one candidate and none equals the value. `count=N` holds if exactly N candidates match
//! (for `exists`: if there are exactly N candidates). `null` is a value: `dock_badge exists` holds
//! when the badge is null, `dock_badge == null` tests for no badge.

use std::fmt;

use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
enum Segment {
    Key(String),
    Index(usize),
    All,
    Filter(String, Value),
    /// `[?field contains value]`.
    FilterContains(String, Value),
}

#[derive(Debug, Clone, PartialEq)]
enum Op {
    Eq(Value),
    Ne(Value),
    Contains(Value),
    Exists,
    NotExists,
}

#[derive(Debug, Clone, PartialEq)]
struct Clause {
    path: Vec<Segment>,
    op: Op,
    count: Option<usize>,
}

/// A parsed condition; see the module docs.
#[derive(Debug, Clone, PartialEq)]
pub struct Condition {
    clauses: Vec<Clause>,
}

/// Where and why a condition does not parse. `column` counts characters from 0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub source: String,
    pub column: usize,
    pub message: String,
}

impl fmt::Display for ParseError {
    /// The message, then the condition with a caret under the offending character.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}\n  {}\n  {}^", self.message, self.source, " ".repeat(self.column))
    }
}

pub fn parse(src: &str) -> Result<Condition, ParseError> {
    let mut p = Parser { src, pos: 0 };
    let mut clauses = vec![p.clause()?];
    loop {
        p.skip_ws();
        if p.rest().is_empty() {
            return Ok(Condition { clauses });
        }
        if !p.eat("&&") {
            return Err(p.error("expected && or count=N"));
        }
        clauses.push(p.clause()?);
    }
}

impl Condition {
    pub fn eval(&self, state: &Value) -> bool {
        self.clauses.iter().all(|c| c.holds(state))
    }
}

/// A parsed path on its own (`gilvt debug eval --path`).
#[derive(Debug, Clone, PartialEq)]
pub struct Path(Vec<Segment>);

/// Parses a whole string as one path (surrounding whitespace allowed).
pub fn parse_path(src: &str) -> Result<Path, ParseError> {
    let mut p = Parser { src, pos: 0 };
    p.skip_ws();
    let path = p.path()?;
    p.skip_ws();
    if !p.rest().is_empty() {
        return Err(p.error("expected the end of the path"));
    }
    Ok(Path(path))
}

impl Path {
    /// The values the path yields in `state`, in order.
    pub fn candidates<'a>(&self, state: &'a Value) -> Vec<&'a Value> {
        candidates(&self.0, state)
    }
}

impl Clause {
    fn holds(&self, state: &Value) -> bool {
        let found = candidates(&self.path, state);
        let matching = |test: &dyn Fn(&Value) -> bool| found.iter().filter(|v| test(v)).count();
        let n = match &self.op {
            Op::Exists => found.len(),
            Op::NotExists => return found.is_empty(),
            Op::Eq(want) => matching(&|v| json_eq(v, want)),
            Op::Contains(want) => matching(&|v| contains(v, want)),
            Op::Ne(want) => {
                let n = matching(&|v| !json_eq(v, want));
                if self.count.is_none() {
                    return !found.is_empty() && n == found.len();
                }
                n
            }
        };
        match self.count {
            Some(count) => n == count,
            None => n > 0,
        }
    }
}

fn candidates<'a>(path: &[Segment], root: &'a Value) -> Vec<&'a Value> {
    let mut now = vec![root];
    for seg in path {
        now = now
            .into_iter()
            .flat_map(|v| -> Vec<&'a Value> {
                match (seg, v) {
                    (Segment::Key(k), Value::Object(map)) => map.get(k).into_iter().collect(),
                    (Segment::Index(i), Value::Array(items)) => items.get(*i).into_iter().collect(),
                    (Segment::All, Value::Array(items)) => items.iter().collect(),
                    (Segment::Filter(field, want), Value::Array(items)) => {
                        items.iter().filter(|item| item.get(field).is_some_and(|x| json_eq(x, want))).collect()
                    }
                    (Segment::FilterContains(field, want), Value::Array(items)) => {
                        items.iter().filter(|item| item.get(field).is_some_and(|x| contains(x, want))).collect()
                    }
                    _ => Vec::new(),
                }
            })
            .collect();
    }
    now
}

/// JSON equality with numbers compared by value, so `1` equals `1.0`; otherwise strict (`true` never
/// equals `1`), recursing into arrays and objects. tests/gui/lib/guilib.py's `json_eq` is the same.
fn json_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => match (x.as_i64(), y.as_i64(), x.as_u64(), y.as_u64()) {
            (Some(x), Some(y), _, _) => x == y,
            (_, _, Some(x), Some(y)) => x == y,
            _ => x.as_f64() == y.as_f64(),
        },
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(x, y)| json_eq(x, y)),
        (Value::Object(x), Value::Object(y)) => x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| json_eq(v, w))),
        _ => a == b,
    }
}

fn contains(haystack: &Value, needle: &Value) -> bool {
    match (haystack, needle) {
        (Value::String(h), Value::String(n)) => h.contains(n.as_str()),
        (Value::Array(items), _) => items.iter().any(|item| json_eq(item, needle)),
        _ => false,
    }
}

fn is_key_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

struct Parser<'a> {
    src: &'a str,
    /// Byte offset into `src`.
    pos: usize,
}

impl Parser<'_> {
    fn rest(&self) -> &str {
        &self.src[self.pos..]
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn skip_ws(&mut self) {
        let rest = self.rest();
        self.pos += rest.len() - rest.trim_start().len();
    }

    fn eat(&mut self, s: &str) -> bool {
        let ok = self.rest().starts_with(s);
        if ok {
            self.pos += s.len();
        }
        ok
    }

    /// Eats the word `w` only if it is not followed by more key characters.
    fn eat_word(&mut self, w: &str) -> bool {
        let rest = self.rest();
        let ok = rest.starts_with(w) && !rest[w.len()..].chars().next().is_some_and(is_key_char);
        if ok {
            self.pos += w.len();
        }
        ok
    }

    fn error_at(&self, pos: usize, message: &str) -> ParseError {
        ParseError { source: self.src.to_string(), column: self.src[..pos].chars().count(), message: message.to_string() }
    }

    fn error(&self, message: &str) -> ParseError {
        self.error_at(self.pos, message)
    }

    fn key(&mut self) -> Option<String> {
        let len: usize = self.rest().chars().take_while(|&c| is_key_char(c)).map(char::len_utf8).sum();
        (len > 0).then(|| {
            let k = self.rest()[..len].to_string();
            self.pos += len;
            k
        })
    }

    /// One JSON literal. The deserializer reads just the first value, so `1]` and `"a" && …` stop
    /// after the literal.
    fn value(&mut self) -> Result<Value, ParseError> {
        let start = self.pos;
        let mut stream = serde_json::Deserializer::from_str(self.rest()).into_iter::<Value>();
        match stream.next() {
            Some(Ok(v)) => {
                self.pos += stream.byte_offset();
                Ok(v)
            }
            _ => Err(self.error_at(start, "expected a JSON value (\"text\", a number, true, false or null)")),
        }
    }

    fn path(&mut self) -> Result<Vec<Segment>, ParseError> {
        let mut path = Vec::new();
        match self.peek() {
            Some('[') => {}
            _ => path.push(Segment::Key(self.key().ok_or_else(|| self.error("expected a path, e.g. windows[0].key"))?)),
        }
        loop {
            if self.eat(".") {
                path.push(Segment::Key(self.key().ok_or_else(|| self.error("expected a key after ."))?));
            } else if self.eat("[") {
                path.push(self.bracket()?);
            } else {
                return Ok(path);
            }
        }
    }

    /// The inside of `[…]`, after the `[`.
    fn bracket(&mut self) -> Result<Segment, ParseError> {
        let digits: usize = self.rest().chars().take_while(char::is_ascii_digit).count();
        let seg = if digits > 0 {
            let i = self.rest()[..digits].parse().map_err(|_| self.error("index too large"))?;
            self.pos += digits;
            Segment::Index(i)
        } else if self.eat("*") {
            Segment::All
        } else if self.eat("?") {
            let field = self.key().ok_or_else(|| self.error("expected a field name after [?"))?;
            self.skip_ws();
            let substring = if self.eat("==") {
                false
            } else if self.eat_word("contains") {
                true
            } else {
                return Err(self.error("expected == or contains in [?field==value]"));
            };
            self.skip_ws();
            let v = self.value()?;
            self.skip_ws();
            if substring {
                Segment::FilterContains(field, v)
            } else {
                Segment::Filter(field, v)
            }
        } else {
            return Err(self.error("expected an index, * or ?field==value after ["));
        };
        self.skip_ws();
        if !self.eat("]") {
            return Err(self.error("expected ]"));
        }
        Ok(seg)
    }

    fn clause(&mut self) -> Result<Clause, ParseError> {
        self.skip_ws();
        let path = self.path()?;
        self.skip_ws();
        let op = if self.eat("==") {
            Op::Eq(self.operand()?)
        } else if self.eat("!=") {
            Op::Ne(self.operand()?)
        } else if self.eat_word("contains") {
            Op::Contains(self.operand()?)
        } else if self.eat_word("!exists") {
            Op::NotExists
        } else if self.eat_word("exists") {
            Op::Exists
        } else {
            return Err(self.error("expected an operator (==, !=, contains, exists, !exists)"));
        };
        let before = self.pos;
        self.skip_ws();
        let count_at = self.pos;
        let count = if self.pos > before && self.eat_word("count") && self.eat("=") {
            if op == Op::NotExists {
                return Err(self.error_at(count_at, "count=N cannot follow !exists"));
            }
            let digits: usize = self.rest().chars().take_while(char::is_ascii_digit).count();
            let n = self.rest()[..digits].parse().map_err(|_| self.error_at(count_at, "count=N needs a whole number"))?;
            self.pos += digits;
            Some(n)
        } else {
            self.pos = before;
            None
        };
        // A clause ends at whitespace or the end.
        if !self.rest().is_empty() && !self.rest().starts_with(char::is_whitespace) {
            return Err(self.error("expected && or count=N"));
        }
        Ok(Clause { path, op, count })
    }

    /// The value after `==` / `!=` / `contains`, which must be followed by whitespace or the end.
    fn operand(&mut self) -> Result<Value, ParseError> {
        self.skip_ws();
        let start = self.pos;
        let v = self.value()?;
        if !self.rest().is_empty() && !self.rest().starts_with(char::is_whitespace) {
            return Err(self.error_at(start, "expected a JSON value (\"text\", a number, true, false or null)"));
        }
        Ok(v)
    }
}

#[cfg(test)]
#[path = "cond_tests.rs"]
mod tests;
