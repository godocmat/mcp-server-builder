use std::collections::BTreeMap;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
use http::{HeaderMap, HeaderName, HeaderValue};
use mcp_policy::{Effect, Redactor};
use mcp_runtime::{AdapterError, AdapterErrorKind};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use zeroize::Zeroizing;

use crate::{SecretError, SecretRequest, SecretResolver, failed};

const MAX_VALUE_BYTES: usize = 8192;
const MAX_HEADER_BYTES: usize = 16 * 1024;

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
    let token = value
        .split_once(' ')
        .filter(|(scheme, _)| {
            scheme.eq_ignore_ascii_case("Bearer") || scheme.eq_ignore_ascii_case("Basic")
        })
        .map(|(_, token)| token.trim());
    let mut variants = Vec::new();
    for value in [Some(value), token]
        .into_iter()
        .flatten()
        .filter(|value| !value.is_empty())
    {
        variants.push(Zeroizing::new(value.to_owned()));
        let escaped = utf8_percent_encode(value, NON_ALPHANUMERIC).to_string();
        variants.push(Zeroizing::new(escaped.clone()));
        let mut lower_hex = escaped.into_bytes();
        let mut index = 0;
        while index < lower_hex.len() {
            if lower_hex[index] == b'%' {
                lower_hex[index + 1..index + 3].make_ascii_lowercase();
                index += 3;
            } else {
                index += 1;
            }
        }
        variants.push(Zeroizing::new(
            String::from_utf8(lower_hex).expect("percent encoding is UTF-8"),
        ));
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
