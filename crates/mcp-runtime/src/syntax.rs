use std::collections::{BTreeMap, BTreeSet};

use fluent_uri::Uri;
use unicode_normalization::UnicodeNormalization;

pub fn is_key(value: &str) -> bool {
    let mut bytes = value.bytes();
    value.len() <= 64
        && bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
        && bytes
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_-".contains(&byte))
}

pub fn is_protocol_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
}

pub fn is_variable(value: &str) -> bool {
    if is_key(value) {
        return true;
    }
    let mut bytes = value.bytes();
    value.len() <= 64
        && bytes
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

pub fn is_portable_path(value: &str) -> bool {
    value.len() <= 4096 && value.split('/').all(is_portable_segment)
}

fn is_portable_segment(value: &str) -> bool {
    if value.is_empty()
        || value.ends_with(['.', ' '])
        || value.contains(['/', '<', '>', ':', '"', '\\', '|', '?', '*', '{', '}'])
        || value.chars().any(char::is_control)
        || value.encode_utf16().count() > 255
    {
        return false;
    }
    let stem = value.split('.').next().unwrap_or("").trim_end_matches(' ');
    let stem: String = stem.nfkc().flat_map(char::to_uppercase).collect();
    if matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" | "CLOCK$"
    ) {
        return false;
    }
    !stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"))
        .is_some_and(|suffix| suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'1'..=b'9'))
}

/// A literal path segment or a single simple {variable} with optional literal affixes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SegmentTemplate {
    Literal(String),
    Variable {
        prefix: String,
        name: String,
        suffix: String,
    },
}

impl SegmentTemplate {
    pub fn parse(value: &str) -> Option<Self> {
        let Some((prefix, rest)) = value.split_once('{') else {
            return (!value.contains('}')).then(|| Self::Literal(value.into()));
        };
        let (name, suffix) = rest.split_once('}')?;
        if !is_variable(name) || prefix.contains('}') || suffix.contains(['{', '}']) {
            return None;
        }
        Some(Self::Variable {
            prefix: prefix.into(),
            name: name.into(),
            suffix: suffix.into(),
        })
    }

    pub fn variable(&self) -> Option<&str> {
        match self {
            Self::Literal(_) => None,
            Self::Variable { name, .. } => Some(name),
        }
    }

    fn expand_example(&self) -> String {
        match self {
            Self::Literal(value) => value.clone(),
            Self::Variable { prefix, suffix, .. } => format!("{prefix}x{suffix}"),
        }
    }

    fn normalize_uri(mut self) -> Option<Self> {
        match &mut self {
            Self::Literal(value) => *value = normalize_uri_part(value)?,
            Self::Variable { prefix, suffix, .. } => {
                *prefix = normalize_uri_part(prefix)?;
                *suffix = normalize_uri_part(suffix)?;
            }
        }
        Some(self)
    }

    pub fn matches(&self, concrete: &str) -> bool {
        match self {
            Self::Literal(value) => value == concrete,
            Self::Variable { prefix, suffix, .. } => concrete
                .strip_prefix(prefix)
                .and_then(|rest| rest.strip_suffix(suffix))
                .is_some_and(|variable| !variable.is_empty()),
        }
    }

    fn overlaps(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Literal(value), _) => other.matches(value),
            (_, Self::Literal(value)) => self.matches(value),
            (
                Self::Variable { prefix, suffix, .. },
                Self::Variable {
                    prefix: right_prefix,
                    suffix: right_suffix,
                    ..
                },
            ) => {
                (prefix.starts_with(right_prefix) || right_prefix.starts_with(prefix))
                    && (suffix.ends_with(right_suffix) || right_suffix.ends_with(suffix))
            }
        }
    }
}

pub fn segments(path: &str) -> Option<Vec<SegmentTemplate>> {
    path.split('/').map(SegmentTemplate::parse).collect()
}

pub fn expand_path(parts: &[SegmentTemplate]) -> String {
    parts
        .iter()
        .map(SegmentTemplate::expand_example)
        .collect::<Vec<_>>()
        .join("/")
}

pub fn variables(parts: &[SegmentTemplate]) -> BTreeSet<&str> {
    parts.iter().filter_map(SegmentTemplate::variable).collect()
}

// Normalize percent-encoding within a URI segment without interpreting dots as path navigation.
fn normalize_uri_part(value: &str) -> Option<String> {
    let mut normalized = String::with_capacity(value.len());
    let mut bytes = value.bytes();
    while let Some(byte) = bytes.next() {
        if byte != b'%' {
            normalized.push(char::from(byte));
            continue;
        }
        let high = char::from(bytes.next()?).to_digit(16)?;
        let low = char::from(bytes.next()?).to_digit(16)?;
        let decoded = char::from(u8::try_from(high * 16 + low).ok()?);
        if decoded.is_ascii_alphanumeric() || "-._~".contains(decoded) {
            normalized.push(decoded);
        } else {
            use std::fmt::Write as _;
            write!(&mut normalized, "%{:02X}", u32::from(decoded)).ok()?;
        }
    }
    Some(normalized)
}

#[derive(Clone, Debug)]
pub struct ResourceAddress {
    prefix: String,
    path: Vec<SegmentTemplate>,
    suffix: String,
}

impl ResourceAddress {
    pub fn parse(value: &str, template: bool) -> Option<Self> {
        if !value.is_ascii() || value.len() > 2048 {
            return None;
        }
        let (scheme, after_scheme) = value.split_once(':')?;
        let end_of_path = after_scheme.find(['?', '#']).unwrap_or(after_scheme.len());
        let (authority_path, suffix) = after_scheme.split_at(end_of_path);
        let (authority, path) = match authority_path.strip_prefix("//") {
            Some(rest) => authority_path.split_at(2 + rest.find('/').unwrap_or(rest.len())),
            None => ("", authority_path),
        };
        let parts = segments(path)?;
        let variable_count = parts
            .iter()
            .filter(|segment| segment.variable().is_some())
            .count();
        if (template && (variable_count == 0 || variable_count != variables(&parts).len()))
            || (!template && variable_count != 0)
        {
            return None;
        }
        let example = format!("{scheme}:{authority}{}{suffix}", expand_path(&parts));
        let parsed = Uri::parse(example.as_str()).ok()?;
        if parsed
            .authority()
            .and_then(|authority| authority.userinfo())
            .is_some()
        {
            return None;
        }
        let normalized = parsed.normalize();
        let address = normalized.as_str();
        let before_path = normalized.scheme().as_str().len()
            + 1
            + normalized
                .authority()
                .map_or(0, |authority| 2 + authority.as_str().len());
        let after_path = before_path + normalized.path().as_str().len();
        let parts = if template {
            let normalized_parts: Vec<_> = parts
                .into_iter()
                .map(SegmentTemplate::normalize_uri)
                .collect::<Option<_>>()?;
            if normalized_parts.iter().any(|part| matches!(part, SegmentTemplate::Literal(value) if value == "." || value == "..")) {
                return None;
            }
            normalized_parts
        } else {
            normalized
                .path()
                .as_str()
                .split('/')
                .map(|value| SegmentTemplate::Literal(value.into()))
                .collect()
        };
        Some(Self {
            prefix: address[..before_path].into(),
            path: parts,
            suffix: address[after_path..].into(),
        })
    }

    pub fn variables(&self) -> BTreeSet<&str> {
        variables(&self.path)
    }

    pub fn overlaps(&self, other: &Self) -> bool {
        self.prefix == other.prefix
            && self.suffix == other.suffix
            && self.path.len() == other.path.len()
            && self
                .path
                .iter()
                .zip(&other.path)
                .all(|(left, right)| left.overlaps(right))
    }

    /// Match a parsed concrete address and decode each captured path variable once.
    /// Captures in the pilot profile are portable path segments for read-only file bindings.
    pub(crate) fn capture(&self, concrete: &Self) -> Option<BTreeMap<String, String>> {
        if self.prefix != concrete.prefix
            || self.suffix != concrete.suffix
            || self.path.len() != concrete.path.len()
        {
            return None;
        }
        let mut variables = BTreeMap::new();
        for (template, part) in self.path.iter().zip(&concrete.path) {
            let SegmentTemplate::Literal(value) = part else {
                return None;
            };
            match template {
                SegmentTemplate::Literal(literal) if literal == value => {}
                SegmentTemplate::Literal(_) => return None,
                SegmentTemplate::Variable {
                    prefix,
                    name,
                    suffix,
                } => {
                    let encoded = value.strip_prefix(prefix)?.strip_suffix(suffix)?;
                    let value = percent_encoding::percent_decode_str(encoded)
                        .decode_utf8()
                        .ok()?;
                    if !is_portable_segment(&value) || value.contains('%') {
                        return None;
                    }
                    variables.insert(name.clone(), value.into_owned());
                }
            }
        }
        Some(variables)
    }
}
