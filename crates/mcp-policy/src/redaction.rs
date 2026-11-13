use std::fmt;
use std::io::{self, Write};

use aho_corasick::AhoCorasick;
use percent_encoding::percent_decode_str;
use serde::Serialize;
use serde_json::Value;

use crate::limits::ByteBudget;

pub const REDACTED: &str = "[REDACTED]";
const MAX_VALUE_DEPTH: usize = 128;

/// Sanitize evidence before formatting, tracing, storage or export. Supply every resolved
/// secret (and any known encodings) for the current run. This type deliberately omits Debug.
#[derive(Default)]
pub struct Redactor {
    secrets: Option<AhoCorasick>,
    sensitive_paths: Vec<Vec<String>>,
}

impl Redactor {
    pub const DEFAULT_ENTRY_BYTES: usize = 8 * 1024;

    pub fn new<I, J>(secret_values: I, sensitive_paths: J) -> Result<Self, RedactionError>
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
        J: IntoIterator,
        J::Item: AsRef<str>,
    {
        let values: Vec<_> = secret_values.into_iter().collect();
        let mut patterns: Vec<_> = values
            .iter()
            .map(AsRef::as_ref)
            .filter(|value| !value.is_empty())
            .collect();
        patterns.sort_unstable();
        patterns.dedup();
        let secrets = if patterns.is_empty() {
            None
        } else {
            Some(AhoCorasick::new(patterns).map_err(|_| RedactionError::SecretMatcher)?)
        };
        let mut sensitive_paths: Vec<_> = sensitive_paths
            .into_iter()
            .map(|path| parse_pointer(path.as_ref()))
            .collect::<Result<_, _>>()?;
        sensitive_paths.sort_unstable();
        sensitive_paths.dedup();
        Ok(Self {
            secrets,
            sensitive_paths,
        })
    }

    /// Omit credential-bearing diagnostics and mask known secrets before truncating. Pass a
    /// bounded complete diagnostic, including all of its read chunks.
    pub fn redact_text(&self, text: &str, max_bytes: usize) -> RedactedText {
        RedactedText::bounded(self.sanitize_text(text), max_bytes, false)
    }

    /// Sanitize a previously bounded value for compact JSON evidence. Mask designated RFC 6901
    /// paths and credential fields. Omit credential headers, cookies, OAuth data and session IDs.
    /// Discard the payload if this method returns an error.
    pub fn redact_json(&self, mut value: Value, max_bytes: usize) -> Result<Value, RedactionError> {
        let paths = self
            .sensitive_paths
            .iter()
            .map(Vec::as_slice)
            .collect::<Vec<_>>();
        self.sanitize(&mut value, &paths, 0)?;
        let max_bytes = u64::try_from(max_bytes).unwrap_or(u64::MAX);
        let mut writer = EvidenceBuffer {
            budget: ByteBudget::new(max_bytes),
            bytes: Vec::new(),
        };
        serde_json::to_writer(&mut writer, &value)
            .map_err(|_| RedactionError::ByteLimitExceeded)?;
        // A known value might still be present across fields or in JSON syntax.
        if self
            .secrets
            .as_ref()
            .is_some_and(|matcher| matcher.is_match(&writer.bytes))
        {
            return Err(RedactionError::SensitiveEncoding);
        }
        Ok(value)
    }

    fn sanitize(
        &self,
        value: &mut Value,
        paths: &[&[String]],
        depth: usize,
    ) -> Result<(), RedactionError> {
        if depth > MAX_VALUE_DEPTH {
            return Err(RedactionError::DepthExceeded);
        }
        if paths.iter().any(|path| path.is_empty()) {
            *value = Value::String(REDACTED.into());
            return Ok(());
        }
        match value {
            Value::Object(fields) => {
                // Handle header records with a name and value as well as header maps.
                if fields.iter().any(|(key, value)| {
                    matches!(key.to_ascii_lowercase().as_str(), "name" | "key" | "header")
                        && value
                            .as_str()
                            .is_some_and(|name| field_rule(name) == FieldRule::Omit)
                }) || fields.contains_key("state")
                    && (fields.contains_key("code") || fields.contains_key("error"))
                {
                    *value = Value::Null;
                    return Ok(());
                }
                fields.retain(|key, _| {
                    !self.contains_secret(key) && field_rule(key) != FieldRule::Omit
                });
                for (key, value) in fields.iter_mut() {
                    if field_rule(key) == FieldRule::Mask {
                        *value = Value::String(REDACTED.into());
                    } else {
                        self.sanitize(value, &child_paths(paths, key), depth + 1)?;
                    }
                }
            }
            Value::Array(items) => {
                if items.len() == 2
                    && items[0]
                        .as_str()
                        .is_some_and(|name| field_rule(name) == FieldRule::Omit)
                {
                    *value = Value::Null;
                    return Ok(());
                }
                for (index, value) in items.iter_mut().enumerate() {
                    self.sanitize(value, &child_paths(paths, &index.to_string()), depth + 1)?;
                }
            }
            Value::String(text) => *text = self.sanitize_text(text),
            _ if self.contains_secret(&value.to_string()) => {
                *value = Value::String(REDACTED.into())
            }
            _ => {}
        }
        Ok(())
    }

    fn contains_secret(&self, text: &str) -> bool {
        self.secrets
            .as_ref()
            .is_some_and(|matcher| matcher.is_match(text))
    }

    fn sanitize_text(&self, input: &str) -> String {
        if has_credential_label(input) {
            return if self.contains_secret(REDACTED) {
                String::new()
            } else {
                REDACTED.into()
            };
        }
        let Some(matcher) = &self.secrets else {
            return input.into();
        };
        if !matcher.is_match(input) {
            return input.into();
        }
        let mut matched_bytes = vec![0; input.len() + 1];
        // Merge overlaps so a shorter secret cannot reveal the tail of a longer one.
        for found in matcher.find_overlapping_iter(input) {
            matched_bytes[found.start()] += 1_i64;
            matched_bytes[found.end()] -= 1;
        }
        let mut output = String::with_capacity(input.len());
        let mut covering = 0;
        for (offset, ch) in input.char_indices() {
            let previous = covering;
            covering += matched_bytes[offset];
            if covering == 0 {
                output.push(ch);
            } else if previous == 0 {
                output.push_str(REDACTED);
            }
        }
        if matcher.is_match(&output) {
            String::new()
        } else {
            output
        }
    }
}

fn parse_pointer(pointer: &str) -> Result<Vec<String>, RedactionError> {
    if pointer.is_empty() {
        return Ok(Vec::new());
    }
    pointer
        .strip_prefix('/')
        .ok_or(RedactionError::InvalidPointer)?
        .split('/')
        .map(|part| {
            let mut decoded = String::new();
            let mut chars = part.chars();
            while let Some(ch) = chars.next() {
                if ch == '~' {
                    decoded.push(match chars.next() {
                        Some('0') => '~',
                        Some('1') => '/',
                        _ => return Err(RedactionError::InvalidPointer),
                    });
                } else {
                    decoded.push(ch);
                }
            }
            Ok(decoded)
        })
        .collect()
}

fn child_paths<'a>(paths: &[&'a [String]], key: &str) -> Vec<&'a [String]> {
    paths
        .iter()
        .filter_map(|path| match path.split_first() {
            Some((name, remaining)) if name == key => Some(remaining),
            _ => None,
        })
        .collect()
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum FieldRule {
    Keep,
    Mask,
    Omit,
}

fn field_rule(field: &str) -> FieldRule {
    let name: String = field
        .trim()
        .chars()
        .filter(|ch| !matches!(ch, '-' | '_'))
        .map(|ch| ch.to_ascii_lowercase())
        .collect();
    match name.as_str() {
        "authorization" | "proxyauthorization" | "wwwauthenticate" | "proxyauthenticate"
        | "cookie" | "cookies" | "setcookie" | "mcpsessionid" | "sessionid" | "oauth"
        | "oauthexchange" | "oauthcallback" | "accesstoken" | "refreshtoken" | "idtoken"
        | "clientsecret" | "clientassertion" | "authorizationcode" | "codeverifier"
        | "codechallenge" | "redirecturi" | "granttype" | "apikey" | "xapikey" => FieldRule::Omit,
        "secret" | "password" | "passwd" | "token" | "credential" | "credentials"
        | "privatekey" => FieldRule::Mask,
        _ => FieldRule::Keep,
    }
}

fn has_credential_label(text: &str) -> bool {
    has_plain_credential_label(text)
        || text.contains('%')
            && has_plain_credential_label(&percent_decode_str(text).decode_utf8_lossy())
}

fn has_plain_credential_label(text: &str) -> bool {
    if text.contains("-----BEGIN") && text.contains("PRIVATE KEY-----") {
        return true;
    }
    text.split_inclusive([':', '=']).any(|part| {
        let label = part.strip_suffix([':', '=']).and_then(|field| {
            field
                .trim_end_matches(|ch: char| ch.is_ascii_whitespace() || matches!(ch, '"' | '\''))
                .rsplit(|ch: char| !ch.is_ascii_alphanumeric() && !matches!(ch, '-' | '_'))
                .next()
        });
        label.is_some_and(|label| field_rule(label) != FieldRule::Keep)
    }) || text
        .split_ascii_whitespace()
        .any(|word| word.eq_ignore_ascii_case("bearer"))
        || ["?code=", "&code=", "?state=", "&state="]
            .iter()
            .any(|parameter| text.contains(parameter))
        || text
            .split("://")
            .skip(1)
            .map(|tail| tail.split(['/', '?', '#']).next().unwrap_or(tail))
            .any(|authority| authority.contains('@'))
}

struct EvidenceBuffer {
    budget: ByteBudget,
    bytes: Vec<u8>,
}

impl Write for EvidenceBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.budget.consume(bytes.len()).map_err(io::Error::other)?;
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RedactedText {
    pub text: String,
    pub truncated: bool,
}

impl RedactedText {
    fn bounded(mut text: String, max_bytes: usize, already_truncated: bool) -> Self {
        let truncated = already_truncated || text.len() > max_bytes;
        if text.len() > max_bytes {
            text.truncate(text.floor_char_boundary(max_bytes));
        }
        Self { text, truncated }
    }
}

/// Bounded stderr capture. Append raw chunks and emit only the result of finish.
/// Dropping an oversized capture in full also drops partial secrets at the capture boundary.
pub struct BufferedText {
    raw: Vec<u8>,
    capture_limit: usize,
    overflowed: bool,
}

impl BufferedText {
    pub fn new(capture_limit: usize) -> Self {
        Self {
            raw: Vec::new(),
            capture_limit,
            overflowed: false,
        }
    }

    pub fn push(&mut self, chunk: &[u8]) {
        if self.overflowed {
            return;
        }
        if chunk.len() > self.capture_limit - self.raw.len() {
            self.overflowed = true;
            self.raw.clear();
            return;
        }
        self.raw.extend_from_slice(chunk);
    }

    pub fn finish(self, redactor: &Redactor, max_bytes: usize) -> RedactedText {
        if self.overflowed {
            return RedactedText::bounded(String::new(), max_bytes, true);
        }
        redactor.redact_text(&String::from_utf8_lossy(&self.raw), max_bytes)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RedactionError {
    InvalidPointer,
    SecretMatcher,
    DepthExceeded,
    ByteLimitExceeded,
    SensitiveEncoding,
}

impl fmt::Display for RedactionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidPointer => "redaction.invalid_pointer",
            Self::SecretMatcher => "redaction.secret_matcher_failed",
            Self::DepthExceeded => "redaction.depth_exceeded",
            Self::ByteLimitExceeded => "redaction.byte_limit_exceeded",
            Self::SensitiveEncoding => "redaction.sensitive_encoding",
        })
    }
}

impl std::error::Error for RedactionError {}
