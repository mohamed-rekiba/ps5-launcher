//! Strict YAML for the emulator manifests.
//!
//! `serde_norway` alone is not strict enough for a file that users edit (see the tests below):
//!
//! - a duplicate key in a map-valued field (`launch.env`) keeps the last value silently;
//! - a global tag (`!!python/object`) is dropped silently;
//! - anchors and aliases are expanded, bounded only by a repetition limit that still lets a
//!   256 KiB file grow into millions of nodes.
//!
//! So a manifest goes through four gates before its typed parse: a size limit, a text scan that
//! rejects anchors, aliases, tags and directives, and a parse into `serde_norway::Value`, whose
//! `Mapping` rejects duplicate keys at every depth. The typed parse then reads the original text,
//! so its errors keep their line and column.

use serde::de::DeserializeOwned;
use std::fmt;

/// The largest manifest the launcher reads.
pub const MAX_BYTES: usize = 256 * 1024;

/// A YAML problem, with the place in the file when it is known.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct YamlError {
    /// 1-based line and column.
    pub at: Option<(usize, usize)>,
    pub message: String,
}

impl fmt::Display for YamlError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.at {
            // serde_norway's own messages already end with "at line L column C".
            Some((line, column)) if !self.message.contains(" at line ") => write!(f, "line {line} column {column}: {}", self.message),
            _ => f.write_str(&self.message),
        }
    }
}

impl From<serde_norway::Error> for YamlError {
    fn from(e: serde_norway::Error) -> Self {
        YamlError { at: e.location().map(|l| (l.line(), l.column())), message: e.to_string() }
    }
}

fn error(line: usize, column: usize, message: impl Into<String>) -> YamlError {
    YamlError { at: Some((line, column)), message: message.into() }
}

/// Run every gate and return the document as an untyped value (for checks such as the schema
/// version that must run before the typed parse).
pub fn load(text: &str) -> Result<serde_norway::Value, YamlError> {
    if text.len() > MAX_BYTES {
        return Err(YamlError { at: None, message: format!("the file is {} KiB; the limit is {} KiB", text.len().div_ceil(1024), MAX_BYTES / 1024) });
    }
    scan(text)?;
    let value: serde_norway::Value = serde_norway::from_str(text)?;
    if let Some(tag) = first_tag(&value) {
        return Err(YamlError { at: None, message: format!("YAML tags are not allowed (found !{tag})") });
    }
    Ok(value)
}

/// The typed parse. Call it only after `load` accepted the same text.
pub fn typed<T: DeserializeOwned>(text: &str) -> Result<T, YamlError> {
    Ok(serde_norway::from_str(text)?)
}

/// A second line of defence for tags: `!local` tags survive as `Value::Tagged`.
fn first_tag(v: &serde_norway::Value) -> Option<String> {
    use serde_norway::Value;
    match v {
        Value::Tagged(t) => Some(t.tag.to_string().trim_start_matches('!').to_string()),
        Value::Sequence(items) => items.iter().find_map(first_tag),
        Value::Mapping(m) => m.iter().find_map(|(k, v)| first_tag(k).or_else(|| first_tag(v))),
        _ => None,
    }
}

/// Reject the YAML features a manifest does not need and that make it unsafe or ambiguous:
/// anchors (`&a`), aliases (`*a`), tags (`!x`, `!!x`) and directives (`%TAG`).
///
/// These indicators only mean something where a node starts: at the start of a line, after a
/// space, or after `[`, `{` or `,` in a flow collection. Inside a quoted scalar, a comment, a
/// block scalar (`|`, `>`) or a plain scalar that has already started they are ordinary text. The
/// scan follows those rules closely. It errs towards rejecting: a plain scalar that continues on a
/// second line starting with `&`, `*` or `!` is refused, and the message asks for quotes.
fn scan(text: &str) -> Result<(), YamlError> {
    #[derive(PartialEq)]
    enum Mode {
        /// Between nodes: a node may start at the next non-space character.
        Between,
        /// Inside a plain (unquoted) scalar.
        Plain,
        /// After a closing quote or a flow end: a node cannot start until a separator.
        After,
        Single,
        Double,
    }
    let mut mode = Mode::Between;
    let mut flow = 0usize;
    // While inside a block scalar: the indentation of the line that opened it.
    let mut block: Option<usize> = None;
    for (n, line) in text.split('\n').enumerate() {
        let line_no = n + 1;
        let indent = line.len() - line.trim_start_matches(' ').len();
        if let Some(parent) = block {
            if line.trim().is_empty() || indent > parent {
                continue;
            }
            block = None;
        }
        if mode == Mode::Plain || mode == Mode::After {
            mode = Mode::Between;
        }
        if mode == Mode::Between && line.starts_with('%') {
            return Err(error(line_no, 1, "YAML directives (%YAML, %TAG) are not allowed"));
        }
        let chars: Vec<(usize, char)> = line.char_indices().collect();
        let mut i = 0;
        while i < chars.len() {
            let (byte, c) = chars[i];
            let column = line[..byte].chars().count() + 1;
            let next = chars.get(i + 1).map(|&(_, c)| c);
            let blank_next = next.is_none_or(|c| c == ' ' || c == '\t' || c == '\r');
            match mode {
                Mode::Single => {
                    if c == '\'' {
                        if next == Some('\'') {
                            i += 1;
                        } else {
                            mode = Mode::After;
                        }
                    }
                }
                Mode::Double => match c {
                    '\\' => i += 1,
                    '"' => mode = Mode::After,
                    _ => {}
                },
                Mode::Between | Mode::After | Mode::Plain => {
                    let at_start = mode == Mode::Between;
                    match c {
                        ' ' | '\t' | '\r' => {
                            if mode == Mode::After {
                                mode = Mode::Between;
                            }
                        }
                        '#' if at_start || (i > 0 && matches!(chars[i - 1].1, ' ' | '\t')) => break,
                        ':' | '?' | '-' if at_start && blank_next => {}
                        ':' if mode != Mode::Between && (blank_next || (flow > 0 && matches!(next, Some(',' | '[' | ']' | '{' | '}')))) => {
                            mode = Mode::Between;
                        }
                        '[' | '{' if mode != Mode::Plain || flow > 0 => {
                            flow += 1;
                            mode = Mode::Between;
                        }
                        ']' | '}' if flow > 0 => {
                            flow -= 1;
                            mode = Mode::After;
                        }
                        ',' if flow > 0 => mode = Mode::Between,
                        '\'' if at_start => mode = Mode::Single,
                        '"' if at_start => mode = Mode::Double,
                        '&' if at_start => return Err(error(line_no, column, "YAML anchors (&name) are not allowed; put quotes around text that starts with &")),
                        '*' if at_start => return Err(error(line_no, column, "YAML aliases (*name) are not allowed; put quotes around text that starts with *")),
                        '!' if at_start => return Err(error(line_no, column, "YAML tags (!tag) are not allowed; put quotes around text that starts with !")),
                        '|' | '>' if at_start && flow == 0 => {
                            block = Some(indent);
                            break;
                        }
                        _ => {
                            if at_start {
                                mode = Mode::Plain;
                            }
                        }
                    }
                }
            }
            i += 1;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(serde::Deserialize, Debug)]
    #[serde(deny_unknown_fields)]
    struct Probe {
        #[serde(default)]
        name: String,
        #[serde(default)]
        on: bool,
        #[serde(default)]
        env: BTreeMap<String, String>,
    }

    // What serde_norway does by itself: these tests pin the reasons for the gates above.

    #[test]
    fn serde_norway_alone_keeps_the_last_of_two_map_keys() {
        let p: Probe = serde_norway::from_str("env:\n  A: one\n  A: two\n").unwrap();
        assert_eq!(p.env["A"], "two", "a duplicate key in a map-valued field passes silently");
        let e = serde_norway::from_str::<Probe>("name: a\nname: b\n").unwrap_err();
        assert!(e.to_string().contains("duplicate field"), "a struct field is caught by serde: {e}");
    }

    #[test]
    fn serde_norway_alone_drops_a_global_tag() {
        let p: Probe = serde_norway::from_str("name: !!python/object:os.system echo\n").unwrap();
        assert_eq!(p.name, "echo", "the tag is ignored, not refused");
    }

    #[test]
    fn serde_norway_alone_expands_aliases() {
        let p: Probe = serde_norway::from_str("name: &n hello\nenv: {A: *n}\n").unwrap();
        assert_eq!(p.env["A"], "hello", "an alias is expanded; the limit is in a_billion_laughs_is_refused_at_once");
    }

    #[test]
    fn serde_norway_reads_yaml_1_2_booleans_only() {
        // The "Norway problem" of YAML 1.1: `no` is not false here.
        let p: Probe = serde_norway::from_str("name: no\nenv: {A: yes, B: off}\n").unwrap();
        assert_eq!((p.name.as_str(), p.env["A"].as_str(), p.env["B"].as_str()), ("no", "yes", "off"));
        assert!(serde_norway::from_str::<Probe>("on: no\n").is_err(), "a bool field refuses `no`");
        assert!(serde_norway::from_str::<Probe>("on: yes\n").is_err());
        assert!(serde_norway::from_str::<Probe>("on: true\n").unwrap().on);
    }

    // The gates.

    #[test]
    fn duplicate_keys_are_refused_at_every_depth() {
        let e = load("env:\n  A: one\n  A: two\n").unwrap_err();
        assert!(e.message.contains("duplicate entry with key \"A\""), "{e}");
        assert!(load("name: a\nname: b\n").is_err());
        assert!(load("a: {b: {c: 1, c: 2}}\n").is_err());
    }

    #[test]
    fn tags_are_refused() {
        for text in ["name: !!python/object:os.system echo\n", "name: !local x\n", "- !!str 1\n", "[a, !x b]\n", "? !k a\n: b\n"] {
            let e = load(text).unwrap_err();
            assert!(e.message.contains("tags"), "{text:?}: {e}");
        }
    }

    #[test]
    fn anchors_and_aliases_are_refused() {
        assert!(load("a: &x 1\nb: *x\n").unwrap_err().message.contains("anchors"));
        assert!(load("a: [*x]\n").unwrap_err().message.contains("aliases"));
        assert!(load("- &a [1, 2]\n").is_err());
    }

    #[test]
    fn a_billion_laughs_is_refused_at_once() {
        let mut bomb = String::from("a: &a [\"lol\",\"lol\",\"lol\",\"lol\",\"lol\",\"lol\",\"lol\",\"lol\",\"lol\"]\n");
        for (name, prev) in ["b", "c", "d", "e", "f", "g", "h", "i"].iter().zip(["a", "b", "c", "d", "e", "f", "g", "h"]) {
            bomb.push_str(&format!("{name}: &{name} [*{prev},*{prev},*{prev},*{prev},*{prev},*{prev},*{prev},*{prev},*{prev}]\n"));
        }
        let start = std::time::Instant::now();
        assert!(load(&bomb).is_err());
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
        // Without the gate serde_norway stops it too, by its repetition limit.
        let e = serde_norway::from_str::<serde_norway::Value>(&bomb).unwrap_err();
        assert!(e.to_string().contains("repetition limit"), "{e}");
    }

    #[test]
    fn directives_are_refused() {
        assert!(load("%YAML 1.1\n---\na: 1\n").unwrap_err().message.contains("directives"));
        assert!(load("%TAG ! tag:example.com,2000:\n---\na: 1\n").is_err());
    }

    #[test]
    fn the_indicators_are_plain_text_where_no_node_starts() {
        for text in [
            "hint: Tom & Jerry\n",
            "hint: a&b *c !d\n",
            "hint: 'it''s &x *y !z'\n",
            "hint: \"quoted \\\" &x *y !z\"\n",
            "# &x *y !z\nhint: ok # &x *y !z\n",
            "url: https://example.com/new?template=a&b=c\n",
            "args: [--a, \"b c\", 'd']\n",
            "hint: |\n  &x *y !z\n  - [b]\nnext: 1\n",
            "list:\n  - a: 1\n    b: \"x\"\n  - --game\n",
            "key: don't\n",
        ] {
            load(text).unwrap_or_else(|e| panic!("{text:?}: {e}"));
        }
    }

    #[test]
    fn a_plain_continuation_line_that_starts_with_an_indicator_needs_quotes() {
        // Valid YAML (one plain scalar on two lines), refused on purpose: rare, and quoting fixes it.
        assert!(load("hint: one\n  &two\n").is_err());
    }

    #[test]
    fn the_size_is_limited() {
        let big = format!("hint: \"{}\"\n", "x".repeat(MAX_BYTES));
        let e = load(&big).unwrap_err();
        assert!(e.message.contains("the limit is 256 KiB"), "{e}");
    }

    #[test]
    fn errors_say_where() {
        let e = load("a: 1\nb: &x 2\n").unwrap_err();
        assert_eq!(e.at, Some((2, 4)));
        assert_eq!(e.to_string(), "line 2 column 4: YAML anchors (&name) are not allowed; put quotes around text that starts with &");
    }
}
