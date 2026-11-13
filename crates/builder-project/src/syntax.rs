use fluent_uri::UriRef;
use mcp_policy::{HttpMethod, HttpOrigin};
use serde::Deserialize;
use serde::de::value::{Error, StrDeserializer};
use unicode_normalization::UnicodeNormalization;

pub(crate) use mcp_runtime::prompt_variables;
pub(crate) use mcp_runtime::syntax::{
    ResourceAddress, SegmentTemplate, is_key, is_portable_path, is_protocol_name, is_variable,
    variables,
};
use mcp_runtime::syntax::{expand_path, segments};

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

pub(crate) fn http_method(method: &str) -> Option<HttpMethod> {
    HttpMethod::deserialize(StrDeserializer::<Error>::new(method)).ok()
}

pub(crate) fn http_origin(value: &str) -> Option<HttpOrigin> {
    (value.len() <= 2048).then(|| value.parse().ok()).flatten()
}

pub(crate) fn portable_path_key(value: &str) -> String {
    value.nfc().flat_map(char::to_uppercase).nfc().collect()
}

fn decode_http(mut part: SegmentTemplate) -> Option<SegmentTemplate> {
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
    match &mut part {
        SegmentTemplate::Literal(value) => {
            *value = decode(value)?;
            if matches!(value.as_str(), "." | "..") {
                return None;
            }
        }
        SegmentTemplate::Variable { prefix, suffix, .. } => {
            *prefix = decode(prefix)?;
            *suffix = decode(suffix)?;
        }
    }
    Some(part)
}

fn is_within(segment: &SegmentTemplate, broader: &SegmentTemplate) -> bool {
    match (segment, broader) {
        (SegmentTemplate::Literal(value), _) => broader.matches(value),
        (SegmentTemplate::Variable { .. }, SegmentTemplate::Literal(_)) => false,
        (
            SegmentTemplate::Variable { prefix, suffix, .. },
            SegmentTemplate::Variable {
                prefix: base_prefix,
                suffix: base_suffix,
                ..
            },
        ) => prefix.starts_with(base_prefix) && suffix.ends_with(base_suffix),
    }
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
    parts.into_iter().map(decode_http).collect()
}

pub(crate) fn path_is_within(narrower: &[SegmentTemplate], broader: &[SegmentTemplate]) -> bool {
    narrower.len() == broader.len()
        && narrower
            .iter()
            .zip(broader)
            .all(|(part, base)| is_within(part, base))
}
