use std::collections::BTreeMap;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Scalar(String),
    List(Vec<String>),
}

impl Value {
    pub fn as_scalar(&self) -> Option<&str> {
        match self {
            Value::Scalar(text) => Some(text),
            Value::List(_) => None,
        }
    }

    pub fn as_list(&self) -> Vec<&str> {
        match self {
            Value::Scalar(text) => std::vec![text.as_str()],
            Value::List(items) => items.iter().map(String::as_str).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Section {
    pub scalars: Vec<(String, Value)>,
    pub sections: Vec<(String, Section)>,
}

impl Section {
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.scalars
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    }

    pub fn section(&self, name: &str) -> Option<&Section> {
        self.sections
            .iter()
            .find(|(child, _)| child == name)
            .map(|(_, section)| section)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    UnterminatedQuote {
        line: usize,
    },
    UnmatchedSectionBrackets {
        line: usize,
    },
    SectionDepthJump {
        line: usize,
        found: usize,
        parent: usize,
    },
    DuplicateKey {
        line: usize,
        key: String,
    },
    DuplicateSection {
        line: usize,
        name: String,
    },
    MissingEquals {
        line: usize,
    },
    EmptyKey {
        line: usize,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::UnterminatedQuote { line } => {
                write!(f, "line {line}: unterminated quoted value")
            }
            ConfigError::UnmatchedSectionBrackets { line } => {
                write!(f, "line {line}: section brackets do not match")
            }
            ConfigError::SectionDepthJump { line, found, parent } => write!(
                f,
                "line {line}: section nested {found} deep under a section {parent} deep (skipped a level)"
            ),
            ConfigError::DuplicateKey { line, key } => {
                write!(f, "line {line}: duplicate key '{key}'")
            }
            ConfigError::DuplicateSection { line, name } => {
                write!(f, "line {line}: duplicate section '{name}'")
            }
            ConfigError::MissingEquals { line } => {
                write!(f, "line {line}: expected 'key = value'")
            }
            ConfigError::EmptyKey { line } => write!(f, "line {line}: empty key name"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl ConfigError {
    pub fn line(&self) -> usize {
        match self {
            ConfigError::UnterminatedQuote { line }
            | ConfigError::UnmatchedSectionBrackets { line }
            | ConfigError::SectionDepthJump { line, .. }
            | ConfigError::DuplicateKey { line, .. }
            | ConfigError::DuplicateSection { line, .. }
            | ConfigError::MissingEquals { line }
            | ConfigError::EmptyKey { line } => *line,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SourceLocations {
    lines: BTreeMap<Vec<String>, usize>,
}

impl SourceLocations {
    pub fn line<I, S>(&self, path: I) -> Option<usize>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let path = path
            .into_iter()
            .map(|part| part.as_ref().to_string())
            .collect::<Vec<_>>();
        self.lines.get(&path).copied()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedConfigObj {
    pub root: Section,
    pub locations: SourceLocations,
}

struct Frame {
    name: String,
    depth: usize,
    section: Section,
}

#[derive(Default)]
struct SectionStack {
    root: Section,
    open: Vec<Frame>,
}

impl SectionStack {
    fn current_depth(&self) -> usize {
        self.open.last().map_or(0, |frame| frame.depth)
    }

    fn current_section(&self) -> &Section {
        self.open.last().map_or(&self.root, |frame| &frame.section)
    }

    fn current_section_mut(&mut self) -> &mut Section {
        self.open
            .last_mut()
            .map_or(&mut self.root, |frame| &mut frame.section)
    }

    fn open(&mut self, name: String, depth: usize) {
        self.open.push(Frame {
            name,
            depth,
            section: Section::default(),
        });
    }

    fn close_to(&mut self, target_depth: usize) {
        while self
            .open
            .last()
            .is_some_and(|frame| frame.depth > target_depth)
        {
            let Some(frame) = self.open.pop() else {
                break;
            };
            self.current_section_mut()
                .sections
                .push((frame.name, frame.section));
        }
    }

    fn finish(mut self) -> Section {
        self.close_to(0);
        self.root
    }
}

pub fn parse(input: &str) -> Result<Section, ConfigError> {
    parse_located(input).map(|parsed| parsed.root)
}

pub fn parse_located(input: &str) -> Result<ParsedConfigObj, ConfigError> {
    let mut stack = SectionStack::default();
    let mut locations = SourceLocations::default();
    let mut current_path = Vec::new();

    let mut lines = input.lines().enumerate();
    while let Some((index, raw_line)) = lines.next() {
        let line_no = index + 1;
        let trimmed = raw_line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        if trimmed.starts_with('[') {
            let (name, depth) = parse_section_header(trimmed, line_no)?;
            stack.close_to(depth - 1);
            let parent_depth = stack.current_depth();
            if depth != parent_depth + 1 {
                return Err(ConfigError::SectionDepthJump {
                    line: line_no,
                    found: depth,
                    parent: parent_depth,
                });
            }
            if stack.current_section().section(&name).is_some() {
                return Err(ConfigError::DuplicateSection {
                    line: line_no,
                    name,
                });
            }
            stack.open(name.clone(), depth);
            current_path.truncate(depth - 1);
            current_path.push(name);
            locations.lines.insert(current_path.clone(), line_no);
            continue;
        }

        let (key, value) = parse_key_value(raw_line, line_no, &mut lines)?;
        let current = stack.current_section_mut();
        if current.get(&key).is_some() {
            return Err(ConfigError::DuplicateKey { line: line_no, key });
        }
        let mut key_path = current_path.clone();
        key_path.push(key.clone());
        locations.lines.insert(key_path, line_no);
        current.scalars.push((key, value));
    }

    Ok(ParsedConfigObj {
        root: stack.finish(),
        locations,
    })
}

fn parse_section_header(trimmed: &str, line_no: usize) -> Result<(String, usize), ConfigError> {
    let depth = trimmed.chars().take_while(|c| *c == '[').count();
    let close_run = "]".repeat(depth);
    let after_open = &trimmed[depth..];
    let close_at = after_open
        .find(&close_run)
        .ok_or(ConfigError::UnmatchedSectionBrackets { line: line_no })?;
    let name = after_open[..close_at].trim();
    let tail = after_open[close_at + depth..].trim_start();
    if !tail.is_empty() && !tail.starts_with('#') {
        return Err(ConfigError::UnmatchedSectionBrackets { line: line_no });
    }
    Ok((unquote(name).to_string(), depth))
}

fn parse_key_value<'a, I>(
    raw_line: &str,
    line_no: usize,
    lines: &mut I,
) -> Result<(String, Value), ConfigError>
where
    I: Iterator<Item = (usize, &'a str)>,
{
    let equals = raw_line
        .find('=')
        .ok_or(ConfigError::MissingEquals { line: line_no })?;
    let key = unquote(raw_line[..equals].trim());
    if key.is_empty() {
        return Err(ConfigError::EmptyKey { line: line_no });
    }
    let mut value_text = raw_line[equals + 1..].to_string();
    while unterminated_triple(&value_text).is_some() {
        let (_, next) = lines
            .next()
            .ok_or(ConfigError::UnterminatedQuote { line: line_no })?;
        value_text.push('\n');
        value_text.push_str(next);
    }
    Ok((key.to_string(), parse_value(&value_text, line_no)?))
}

fn unterminated_triple(value_text: &str) -> Option<&'static str> {
    let trimmed = value_text.trim_start();
    for delimiter in ["\"\"\"", "'''"] {
        if let Some(rest) = trimmed.strip_prefix(delimiter) {
            if !rest.contains(delimiter) {
                return Some(delimiter);
            }
        }
    }
    None
}

fn parse_value(raw: &str, line_no: usize) -> Result<Value, ConfigError> {
    for delimiter in ["\"\"\"", "'''"] {
        let trimmed = raw.trim();
        if let Some(rest) = trimmed.strip_prefix(delimiter) {
            let end = rest
                .find(delimiter)
                .ok_or(ConfigError::UnterminatedQuote { line: line_no })?;
            return Ok(Value::Scalar(rest[..end].to_string()));
        }
    }

    let mut elements = Vec::new();
    let mut current = String::new();
    let mut had_comma = false;
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' | '"' => {
                for inner in chars.by_ref() {
                    if inner == c {
                        break;
                    }
                    current.push(inner);
                }
            }
            '#' => break,
            ',' => {
                had_comma = true;
                elements.push(current.trim().to_string());
                current = String::new();
            }
            other => current.push(other),
        }
    }
    let tail = current.trim();
    if !tail.is_empty() || (had_comma && elements.is_empty()) {
        elements.push(tail.to_string());
    }

    if had_comma {
        elements.retain(|element| !element.is_empty());
        Ok(Value::List(elements))
    } else {
        Ok(Value::Scalar(
            elements.into_iter().next().unwrap_or_default(),
        ))
    }
}

fn unquote(text: &str) -> &str {
    let bytes = text.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        if (first == b'\'' || first == b'"') && bytes[bytes.len() - 1] == first {
            return &text[1..text.len() - 1];
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scalar(section: &Section, key: &str) -> String {
        section
            .get(key)
            .and_then(Value::as_scalar)
            .unwrap()
            .to_string()
    }

    #[test]
    fn nested_sections_track_bracket_depth() {
        let root = parse(
            "[reticulum]\n\
             share_instance = Yes\n\
             [interfaces]\n\
               [[Default Interface]]\n\
                 type = AutoInterface\n\
                 enabled = Yes\n",
        )
        .unwrap();
        assert_eq!(
            scalar(root.section("reticulum").unwrap(), "share_instance"),
            "Yes"
        );
        let interfaces = root.section("interfaces").unwrap();
        let default = interfaces.section("Default Interface").unwrap();
        assert_eq!(scalar(default, "type"), "AutoInterface");
    }

    #[test]
    fn triple_nested_subinterfaces_attach_to_their_parent() {
        let root = parse(
            "[interfaces]\n\
               [[Radio]]\n\
                 type = RNodeMultiInterface\n\
                 [[[Sub A]]]\n\
                   vport = 0\n\
                 [[[Sub B]]]\n\
                   vport = 1\n",
        )
        .unwrap();
        let radio = root
            .section("interfaces")
            .unwrap()
            .section("Radio")
            .unwrap();
        assert_eq!(radio.sections.len(), 2);
        assert_eq!(scalar(radio.section("Sub A").unwrap(), "vport"), "0");
        assert_eq!(scalar(radio.section("Sub B").unwrap(), "vport"), "1");
    }

    #[test]
    fn comma_values_become_lists_and_lone_values_stay_scalar() {
        let root = parse("[x]\ndevices = eth0, wlan0\nsingle = eth0\ntrailing = eth0,\n").unwrap();
        let x = root.section("x").unwrap();
        assert_eq!(
            x.get("devices").unwrap().as_list(),
            std::vec!["eth0", "wlan0"]
        );
        assert_eq!(x.get("single").unwrap(), &Value::Scalar("eth0".to_string()));
        assert_eq!(
            x.get("trailing").unwrap(),
            &Value::List(std::vec!["eth0".to_string()])
        );
    }

    #[test]
    fn inline_and_full_line_comments_are_stripped() {
        let root = parse("# top comment\n[x]\nkey = value  # trailing\n").unwrap();
        assert_eq!(scalar(root.section("x").unwrap(), "key"), "value");
    }

    #[test]
    fn a_hash_inside_quotes_is_not_a_comment() {
        let root = parse("[x]\npassphrase = \"a # b\"\n").unwrap();
        assert_eq!(scalar(root.section("x").unwrap(), "passphrase"), "a # b");
    }

    #[test]
    fn quoted_list_elements_keep_their_commas() {
        let root = parse("[x]\npeers = \"a, b\", c\n").unwrap();
        assert_eq!(
            root.section("x").unwrap().get("peers").unwrap().as_list(),
            std::vec!["a, b", "c"]
        );
    }

    #[test]
    fn a_section_depth_jump_is_an_error() {
        let result = parse("[a]\n[[[c]]]\n");
        assert!(matches!(
            result,
            Err(ConfigError::SectionDepthJump {
                found: 3,
                parent: 1,
                ..
            })
        ));
    }

    #[test]
    fn mismatched_section_brackets_are_an_error() {
        assert!(matches!(
            parse("[[foo]\n"),
            Err(ConfigError::UnmatchedSectionBrackets { .. })
        ));
    }

    #[test]
    fn duplicate_keys_are_rejected() {
        assert!(matches!(
            parse("[x]\nkey = 1\nkey = 2\n"),
            Err(ConfigError::DuplicateKey { .. })
        ));
    }

    #[test]
    fn a_multi_line_triple_quoted_value_is_joined() {
        let root = parse("[x]\nbanner = '''line one\nline two'''\n").unwrap();
        assert_eq!(
            scalar(root.section("x").unwrap(), "banner"),
            "line one\nline two"
        );
    }

    #[test]
    fn located_parse_tracks_full_section_and_key_paths() {
        let parsed = parse_located(
            "[reticulum]\nenable_transport = Yes\n[interfaces]\n[[Hub]]\ntype = TCPClientInterface\n",
        )
        .unwrap();
        assert_eq!(parsed.locations.line(["reticulum"]), Some(1));
        assert_eq!(
            parsed.locations.line(["reticulum", "enable_transport"]),
            Some(2)
        );
        assert_eq!(parsed.locations.line(["interfaces", "Hub"]), Some(4));
        assert_eq!(
            parsed.locations.line(["interfaces", "Hub", "type"]),
            Some(5)
        );
    }
}
