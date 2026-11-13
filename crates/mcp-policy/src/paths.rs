use std::collections::BTreeMap;
use std::fmt;

use percent_encoding::percent_decode_str;

#[derive(Clone, Copy)]
pub(crate) enum PathKind {
    Http,
    File,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PathError;

impl fmt::Display for PathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid policy path")
    }
}

impl std::error::Error for PathError {}

pub(crate) fn is_key(value: &str) -> bool {
    let mut bytes = value.bytes();
    value.len() <= 64
        && bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
        && bytes
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_-".contains(&byte))
}

pub(crate) fn is_entry(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn is_variable(value: &str) -> bool {
    is_entry(value) || is_key(value)
}

#[derive(Debug)]
enum Pattern {
    Literal(String),
    Variable {
        name: String,
        prefix: String,
        suffix: String,
    },
}

impl Pattern {
    fn matches<'a>(&'a self, segment: &'a str, captures: &mut BTreeMap<&'a str, &'a str>) -> bool {
        match self {
            Self::Literal(literal) => segment == literal,
            Self::Variable {
                name,
                prefix,
                suffix,
            } => segment
                .strip_prefix(prefix)
                .and_then(|rest| rest.strip_suffix(suffix))
                .is_some_and(|value| {
                    !value.is_empty() && *captures.entry(name.as_str()).or_insert(value) == value
                }),
        }
    }
}

#[derive(Debug)]
pub(crate) struct PathTemplate(Vec<Pattern>);

impl PathTemplate {
    pub(crate) fn parse(value: &str, kind: PathKind) -> Result<Self, PathError> {
        let segments = path_segments(value, kind)?
            .map(|segment| parse_pattern(segment, kind))
            .collect::<Result<_, _>>()?;
        Ok(Self(segments))
    }

    pub(crate) fn matches(&self, path: &[String]) -> bool {
        let mut captures = BTreeMap::new();
        self.0.len() == path.len()
            && self
                .0
                .iter()
                .zip(path)
                .all(|(pattern, segment)| pattern.matches(segment, &mut captures))
    }
}

pub(crate) fn concrete_path(value: &str, kind: PathKind) -> Result<Vec<String>, PathError> {
    if matches!(kind, PathKind::Http) && value.chars().any(char::is_whitespace) {
        return Err(PathError);
    }
    path_segments(value, kind)?
        .map(|segment| decode_segment(segment, kind, false))
        .collect()
}

fn path_segments(value: &str, kind: PathKind) -> Result<impl Iterator<Item = &str>, PathError> {
    let path = match kind {
        PathKind::Http => value.strip_prefix('/').ok_or(PathError)?,
        PathKind::File if !value.is_empty() => value,
        PathKind::File => return Err(PathError),
    };
    // HTTP root and trailing slashes are significant. File paths have neither.
    let mut segments = path.split('/').peekable();
    while let Some(segment) = segments.next() {
        if segment.is_empty() && (matches!(kind, PathKind::File) || segments.peek().is_some()) {
            return Err(PathError);
        }
    }
    Ok(path.split('/'))
}

fn parse_pattern(segment: &str, kind: PathKind) -> Result<Pattern, PathError> {
    let Some((prefix, variable)) = segment.split_once('{') else {
        return decode_segment(segment, kind, false).map(Pattern::Literal);
    };
    let (name, suffix) = variable.split_once('}').ok_or(PathError)?;
    if !is_variable(name) {
        return Err(PathError);
    }
    Ok(Pattern::Variable {
        name: name.into(),
        prefix: decode_segment(prefix, kind, true)?,
        suffix: decode_segment(suffix, kind, true)?,
    })
}

fn decode_segment(segment: &str, kind: PathKind, fragment: bool) -> Result<String, PathError> {
    let decoded = match kind {
        PathKind::Http => {
            for (index, _) in segment.match_indices('%') {
                let bytes = segment.as_bytes();
                if !bytes
                    .get(index + 1..index + 3)
                    .is_some_and(|hex| hex.iter().all(u8::is_ascii_hexdigit))
                {
                    return Err(PathError);
                }
            }
            percent_decode_str(segment)
                .decode_utf8()
                .map_err(|_| PathError)?
        }
        PathKind::File => segment.into(),
    };
    if decoded.chars().any(|ch| {
        ch.is_control()
            || match kind {
                PathKind::Http => "/\\?#%;{}".contains(ch),
                PathKind::File => "/\\<>:\"|?*{}".contains(ch),
            }
    }) {
        return Err(PathError);
    }
    if !fragment
        && (matches!(decoded.as_ref(), "." | "..")
            || matches!(kind, PathKind::File) && invalid_file_name(&decoded))
    {
        return Err(PathError);
    }
    Ok(decoded.into_owned())
}

fn invalid_file_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).trim_end_matches(' ');
    let stem = stem.to_ascii_uppercase();
    name.ends_with(['.', ' '])
        || matches!(
            stem.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        )
        || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|number| {
                matches!(
                    number,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
}
