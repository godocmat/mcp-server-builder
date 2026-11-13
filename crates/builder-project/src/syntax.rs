use std::collections::BTreeSet;

use fluent_uri::{Uri, UriRef};
use unicode_normalization::UnicodeNormalization;
use url::Url;

use crate::GrantOperation;

pub(crate) fn is_key(value: &str) -> bool {
    let mut bytes = value.bytes();
    value.len() <= 64
        && bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
        && bytes
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_-".contains(&byte))
}

pub(crate) fn is_protocol_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
}

pub(crate) fn is_variable(value: &str) -> bool {
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

pub(crate) fn is_pointer(value: &str) -> bool {
    if value.len() > 2048 || (!value.is_empty() && !value.starts_with('/')) {
        return false;
    }
    let mut bytes = value.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'~' && !matches!(bytes.next(), Some(b'0' | b'1')) {
            return false;
        }
    }
    !value.chars().any(char::is_control)
}

pub(crate) fn push_pointer(pointer: &mut String, segment: &str) {
    pointer.push('/');
    pointer.push_str(&segment.replace('~', "~0").replace('/', "~1"));
}

pub(crate) fn field(parent: &str, segment: &str) -> String {
    let mut pointer = parent.to_owned();
    push_pointer(&mut pointer, segment);
    pointer
}

pub(crate) fn http_operation(method: &str) -> Option<GrantOperation> {
    match method {
        "GET" | "HEAD" | "OPTIONS" => Some(GrantOperation::HttpRead),
        "POST" | "PUT" | "PATCH" | "DELETE" => Some(GrantOperation::HttpWrite),
        _ => None,
    }
}

pub(crate) fn http_origin(value: &str) -> Option<url::Origin> {
    let (_, authority) = value.strip_suffix('/').unwrap_or(value).split_once("://")?;
    if authority.is_empty()
        || authority.contains(['/', '\\', '@', '?', '#', '%'])
        || value.len() > 2048
        || value
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return None;
    }
    let url = Url::parse(value).ok()?;
    (matches!(url.scheme(), "http" | "https") && url.has_host() && url.port() != Some(0))
        .then(|| url.origin())
}

pub fn is_portable_path(value: &str) -> bool {
    value.len() <= 4096 && value.split('/').all(portable_segment)
}

fn portable_segment(value: &str) -> bool {
    if value.is_empty()
        || value.ends_with(['.', ' '])
        || value.contains(['<', '>', ':', '"', '\\', '|', '?', '*', '{', '}'])
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

pub(crate) fn portable_path_key(value: &str) -> String {
    value.nfc().flat_map(char::to_uppercase).nfc().collect()
}

/// A literal path segment or a single simple {variable} with optional literal affixes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SegmentTemplate {
    Literal(String),
    Variable {
        prefix: String,
        name: String,
        suffix: String,
    },
}

impl SegmentTemplate {
    fn parse(value: &str) -> Option<Self> {
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

    fn variable(&self) -> Option<&str> {
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

    fn decode_http(mut self) -> Option<Self> {
        fn decode(value: &str) -> Option<String> {
            for (index, _) in value.match_indices('%') {
                if !value
                    .as_bytes()
                    .get(index + 1..index + 3)
                    .is_some_and(|digits| digits.iter().all(u8::is_ascii_hexdigit))
                {
                    return None;
                }
            }
            let decoded = percent_encoding::percent_decode_str(value)
                .decode_utf8()
                .ok()?;
            (!decoded
                .chars()
                .any(|ch| ch.is_control() || "/\\?#%;{}".contains(ch)))
            .then(|| decoded.into_owned())
        }
        match &mut self {
            Self::Literal(value) => {
                *value = decode(value)?;
                if matches!(value.as_str(), "." | "..") {
                    return None;
                }
            }
            Self::Variable { prefix, suffix, .. } => {
                *prefix = decode(prefix)?;
                *suffix = decode(suffix)?;
            }
        }
        Some(self)
    }

    fn matches(&self, concrete: &str) -> bool {
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

    fn is_within(&self, broader: &Self) -> bool {
        match (self, broader) {
            (Self::Literal(value), _) => broader.matches(value),
            (Self::Variable { .. }, Self::Literal(_)) => false,
            (
                Self::Variable { prefix, suffix, .. },
                Self::Variable {
                    prefix: base_prefix,
                    suffix: base_suffix,
                    ..
                },
            ) => prefix.starts_with(base_prefix) && suffix.ends_with(base_suffix),
        }
    }
}

fn segments(path: &str) -> Option<Vec<SegmentTemplate>> {
    path.split('/').map(SegmentTemplate::parse).collect()
}

pub(crate) fn file_template(path: &str) -> Option<Vec<SegmentTemplate>> {
    if path.len() > 4096 {
        return None;
    }
    let parts = segments(path)?;
    let example = expand_path(&parts);
    is_portable_path(&example).then_some(parts)
}

pub(crate) fn rest_template(path: &str) -> Option<Vec<SegmentTemplate>> {
    if path.len() > 2048 || !path.starts_with('/') || path.contains("//") {
        return None;
    }
    let parts = segments(path)?;
    let example = expand_path(&parts);
    let uri = UriRef::parse(example.as_str()).ok()?;
    if uri.query().is_some() || uri.fragment().is_some() {
        return None;
    }
    parts
        .into_iter()
        .map(SegmentTemplate::decode_http)
        .collect()
}

fn expand_path(parts: &[SegmentTemplate]) -> String {
    parts
        .iter()
        .map(SegmentTemplate::expand_example)
        .collect::<Vec<_>>()
        .join("/")
}

pub(crate) fn variables(parts: &[SegmentTemplate]) -> BTreeSet<&str> {
    parts.iter().filter_map(SegmentTemplate::variable).collect()
}

pub(crate) fn path_is_within(narrower: &[SegmentTemplate], broader: &[SegmentTemplate]) -> bool {
    narrower.len() == broader.len()
        && narrower
            .iter()
            .zip(broader)
            .all(|(part, base)| part.is_within(base))
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

#[derive(Debug)]
pub(crate) struct ResourceAddress {
    prefix: String,
    path: Vec<SegmentTemplate>,
    suffix: String,
}

impl ResourceAddress {
    pub(crate) fn parse(value: &str, template: bool) -> Option<Self> {
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

    pub(crate) fn variables(&self) -> BTreeSet<&str> {
        variables(&self.path)
    }

    pub(crate) fn overlaps(&self, other: &Self) -> bool {
        self.prefix == other.prefix
            && self.suffix == other.suffix
            && self.path.len() == other.path.len()
            && self
                .path
                .iter()
                .zip(&other.path)
                .all(|(left, right)| left.overlaps(right))
    }
}

pub(crate) fn prompt_variables(mut text: &str) -> Option<BTreeSet<&str>> {
    let mut names = BTreeSet::new();
    while let Some(index) = text.find(['{', '}']) {
        text = &text[index..];
        if let Some(rest) = text.strip_prefix("{{").or_else(|| text.strip_prefix("}}")) {
            text = rest;
            continue;
        }
        let (name, rest) = text.strip_prefix('{')?.split_once('}')?;
        if !is_variable(name) {
            return None;
        }
        names.insert(name);
        text = rest;
    }
    Some(names)
}
