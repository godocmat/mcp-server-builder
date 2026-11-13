use std::collections::BTreeMap;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
use http::{HeaderMap, HeaderName, HeaderValue};
use mcp_policy::{Effect, Redactor};
use mcp_runtime::{AdapterError, AdapterErrorKind};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use zeroize::Zeroizing;

use crate::{SecretError, SecretRequest, SecretResolver, failed};

const MAX_VALUE_BYTES: usize = 8192;
const MAX_HEADER_BYTES: usize = 16 * 1024;
const URI_ESCAPE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

pub(super) struct Credentials {
    pub(super) headers: HeaderMap,
    pub(super) redactor: Redactor,
}

pub(super) async fn resolve<'a>(
    resolver: Option<&dyn SecretResolver>,
    names: &'a [(HeaderName, String)],
    scope: impl Fn(&'a str) -> SecretRequest<'a>,
) -> Result<Credentials, AdapterError> {
    let mut headers = HeaderMap::new();
    let mut values = BTreeMap::new();
    let mut total_bytes = 0;
    for (name, reference) in names {
        let value = if let Some(value) = values.get(reference.as_str()) {
            value
        } else {
            let resolver = resolver.ok_or_else(|| error(AdapterErrorKind::CredentialMissing))?;
            let value = resolver
                .resolve(scope(reference))
                .await
                .map_err(|error_kind| {
                    error(match error_kind {
                        SecretError::Missing => AdapterErrorKind::CredentialMissing,
                        SecretError::Unavailable => AdapterErrorKind::CredentialUnavailable,
                    })
                })?;
            if value.trim().is_empty() || value.len() > MAX_VALUE_BYTES {
                return Err(error(AdapterErrorKind::CredentialInvalid));
            }
            values.entry(reference.as_str()).or_insert(value)
        };
        total_bytes += name.as_str().len() + value.len() + 4;
        if total_bytes > MAX_HEADER_BYTES {
            return Err(error(AdapterErrorKind::CredentialInvalid));
        }
        let mut header =
            HeaderValue::from_str(value).map_err(|_| error(AdapterErrorKind::CredentialInvalid))?;
        header.set_sensitive(true);
        headers.insert(name, header);
    }
    let variants: Vec<_> = values
        .values()
        .flat_map(|value| secret_variants(value))
        .collect();
    let redactor = Redactor::new(
        variants.iter().map(|value| value.as_str()),
        std::iter::empty::<&str>(),
    )
    .map_err(|_| failed(Effect::None))?;
    Ok(Credentials { headers, redactor })
}

fn secret_variants(value: &str) -> Vec<Zeroizing<String>> {
    // HTTP recipients can remove leading or trailing whitespace when parsing
    // a header. Include the value as parsed and the token of known auth schemes.
    let trimmed = value.trim();
    let scheme_and_token = trimmed.split_once(|ch: char| ch.is_ascii_whitespace());
    let token = scheme_and_token
        .filter(|(scheme, _)| {
            scheme.eq_ignore_ascii_case("Bearer") || scheme.eq_ignore_ascii_case("Basic")
        })
        .map(|(_, token)| token.trim());
    let basic = scheme_and_token
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("Basic"))
        .and_then(|(_, token)| STANDARD.decode(token.trim()).ok())
        .and_then(|decoded| String::from_utf8(decoded).ok())
        .map(Zeroizing::new);
    let mut variants = Vec::new();
    for value in [
        Some(value),
        (trimmed != value).then_some(trimmed),
        token,
        basic.as_ref().map(|value| value.as_str()),
    ]
    .into_iter()
    .flatten()
    .filter(|value| !value.is_empty())
    {
        variants.push(Zeroizing::new(value.to_owned()));
        for set in [URI_ESCAPE, NON_ALPHANUMERIC] {
            let escaped = Zeroizing::new(utf8_percent_encode(value, set).to_string());
            let mut lower_hex = escaped.to_string().into_bytes();
            let mut index = 0;
            while index < lower_hex.len() {
                if lower_hex[index] == b'%' {
                    lower_hex[index + 1..index + 3].make_ascii_lowercase();
                    index += 3;
                } else {
                    index += 1;
                }
            }
            variants.push(escaped);
            variants.push(Zeroizing::new(
                String::from_utf8(lower_hex).expect("percent encoding is UTF-8"),
            ));
        }
        for engine in [STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD] {
            variants.push(Zeroizing::new(engine.encode(value.as_bytes())));
        }
    }
    variants
}

fn error(kind: AdapterErrorKind) -> AdapterError {
    AdapterError {
        kind,
        effect: Effect::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_variants_cover_transport_parsing_and_common_echoes() {
        let value = "  Bearer sample.token/key-42  ";
        let variants = secret_variants(value);
        let contains = |text: &str| variants.iter().any(|value| value.as_str() == text);
        for echo in [
            value,
            "Bearer sample.token/key-42",
            "sample.token/key-42",
            "sample.token%2Fkey-42",
            "sample.token%2fkey-42",
            "sample%2Etoken%2Fkey%2D42",
        ] {
            assert!(contains(echo));
        }
        let basic = secret_variants("Basic Zml4dHVyZTpwYXNz");
        assert!(basic.iter().any(|value| value.as_str() == "fixture:pass"));
    }
}
