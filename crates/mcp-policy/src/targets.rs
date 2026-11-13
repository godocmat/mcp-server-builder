use std::fmt;
use std::net::{IpAddr, Ipv4Addr};
use std::str::FromStr;

use ipnet::IpNet;
use serde::{Deserialize, Serialize};
use url::{Host, Url};

use crate::paths::{PathKind, concrete_path, is_entry, is_key};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Head,
    Options,
    Post,
    Put,
    Patch,
    Delete,
}

impl HttpMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Head => "HEAD",
            Self::Options => "OPTIONS",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
        }
    }

    pub fn is_read(self) -> bool {
        matches!(self, Self::Get | Self::Head | Self::Options)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DatabaseAccess {
    Read,
    Write,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GitOperation {
    ShowCommit,
    ReadDiff,
}

/// A parsed HTTP(S) origin with a canonical host and effective port.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct HttpOrigin(Url);

impl HttpOrigin {
    pub fn as_str(&self) -> &str {
        &self.0[..url::Position::BeforePath]
    }

    pub fn scheme(&self) -> &str {
        self.0.scheme()
    }

    pub fn port(&self) -> u16 {
        self.0
            .port_or_known_default()
            .expect("validated HTTP(S) origin")
    }

    /// The canonical DNS name or unbracketed numeric address for DNS and TLS.
    pub fn host(&self) -> &str {
        let host = self.0.host_str().expect("validated HTTP(S) origin");
        host.strip_prefix('[')
            .and_then(|host| host.strip_suffix(']'))
            .unwrap_or(host)
    }

    pub fn authority(&self) -> &str {
        &self.0[url::Position::BeforeHost..url::Position::AfterPort]
    }

    pub fn literal_ip(&self) -> Option<IpAddr> {
        let host = self.0.host()?;
        match host {
            Host::Ipv4(ip) => Some(ip.into()),
            Host::Ipv6(ip) => Some(IpAddr::from(ip).to_canonical()),
            Host::Domain(_) => None,
        }
    }
}

impl FromStr for HttpOrigin {
    type Err = OriginError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (_, authority) = value.split_once("://").ok_or(OriginError)?;
        let authority = authority.strip_suffix('/').unwrap_or(authority);
        if authority.is_empty()
            || authority.contains(['/', '\\', '@', '?', '#', '%'])
            || value
                .chars()
                .any(|ch| ch.is_control() || ch.is_whitespace())
        {
            return Err(OriginError);
        }
        let url = Url::parse(value).map_err(|_| OriginError)?;
        if !matches!(url.scheme(), "http" | "https")
            || !url.has_host()
            || url.port_or_known_default() == Some(0)
        {
            return Err(OriginError);
        }
        Ok(Self(url))
    }
}

impl fmt::Display for HttpOrigin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl TryFrom<String> for HttpOrigin {
    type Error = OriginError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<HttpOrigin> for String {
    fn from(value: HttpOrigin) -> Self {
        value.as_str().into()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OriginError;

impl fmt::Display for OriginError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid HTTP origin")
    }
}

impl std::error::Error for OriginError {}

/// An exact method, origin and origin-form path for an explicitly approved redirect.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestRoute {
    pub origin: HttpOrigin,
    pub method: HttpMethod,
    pub path: String,
}

impl RestRoute {
    pub(crate) fn is_valid(&self) -> bool {
        Self::is_valid_path(&self.path)
    }

    /// Validate a concrete path before a live client uses it in an HTTP request.
    pub fn is_valid_path(path: &str) -> bool {
        concrete_path(path, PathKind::Http).is_ok()
    }
}

/// Facts supplied by trusted callers after checking the relevant handle, peer or binding.
/// These values never come from invocation arguments or deserialized policy.
pub enum VerifiedTarget<'a> {
    Rest {
        origin: &'a HttpOrigin,
        method: HttpMethod,
        path: &'a str,
        peer: IpAddr,
        redirected: bool,
    },
    FileRead {
        root_ref: &'a str,
        relative_path: &'a str,
    },
    Sqlite {
        target_ref: &'a str,
        binding_id: &'a str,
        access: DatabaseAccess,
    },
    Postgres {
        target_ref: &'a str,
        binding_id: &'a str,
        access: DatabaseAccess,
    },
    Git {
        repository_ref: &'a str,
        operation: GitOperation,
    },
    Executable {
        executable_ref: &'a str,
        cwd_ref: &'a str,
        binding_id: &'a str,
    },
    CustomRust {
        source: &'a str,
        entry: &'a str,
    },
}

pub(crate) struct TargetFacts<'a, 'target> {
    pub(crate) target: &'a VerifiedTarget<'target>,
    pub(crate) path: Vec<String>,
}

impl<'target> VerifiedTarget<'target> {
    pub(crate) fn checked(&self) -> Option<TargetFacts<'_, 'target>> {
        let path = match self {
            Self::Rest {
                origin, path, peer, ..
            } if origin
                .literal_ip()
                .is_none_or(|ip| ip == peer.to_canonical()) =>
            {
                concrete_path(path, PathKind::Http).ok()?
            }
            Self::FileRead {
                root_ref,
                relative_path,
            } if is_key(root_ref) => concrete_path(relative_path, PathKind::File).ok()?,
            Self::Sqlite {
                target_ref,
                binding_id,
                ..
            }
            | Self::Postgres {
                target_ref,
                binding_id,
                ..
            } if is_key(target_ref) && is_key(binding_id) => Vec::new(),
            Self::Git { repository_ref, .. } if is_key(repository_ref) => Vec::new(),
            Self::Executable {
                executable_ref,
                cwd_ref,
                binding_id,
            } if is_key(executable_ref) && is_key(cwd_ref) && is_key(binding_id) => Vec::new(),
            Self::CustomRust { source, entry } if is_entry(entry) => {
                concrete_path(source, PathKind::File).ok()?
            }
            _ => return None,
        };
        Some(TargetFacts { target: self, path })
    }
}

/// Check an already verified peer address. Socket selection, DNS, TLS and actual peer
/// verification belong to the REST adapter.
pub(crate) fn allows_peer(ranges: Option<&[IpNet]>, peer: IpAddr) -> bool {
    let peer = peer.to_canonical();
    if peer.is_unspecified() || peer.is_multicast() || peer == IpAddr::V4(Ipv4Addr::BROADCAST) {
        return false;
    }
    ranges.map_or_else(
        || is_public_peer(peer),
        |ranges| ranges.iter().any(|range| range.contains(&peer)),
    )
}

// Treat special-use blocks conservatively, including transition prefixes that can carry IPv4.
fn is_public_peer(peer: IpAddr) -> bool {
    match peer {
        IpAddr::V4(ip) => !matches!(
            ip.octets(),
            [0 | 10 | 127, ..]
                | [100, 64..=127, ..]
                | [169, 254, ..]
                | [172, 16..=31, ..]
                | [192, 0, 0 | 2, _]
                | [192, 88, 99, _]
                | [192, 168, ..]
                | [198, 18 | 19, ..]
                | [198, 51, 100, _]
                | [203, 0, 113, _]
                | [224..=255, ..]
        ),
        IpAddr::V6(ip) => {
            let [first, second, ..] = ip.segments();
            (0x2000..=0x3fff).contains(&first)
                && !matches!(first, 0x2002 | 0x3fff)
                && !(first == 0x2001 && (second < 0x0200 || second == 0x0db8))
        }
    }
}
