use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct ManifestRevision([u8; 32]);

impl ManifestRevision {
    pub fn of(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }
}

impl fmt::Display for ManifestRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "sha256:")?;
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct InvalidRevision;

impl fmt::Display for InvalidRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Expected a sha256: revision with 64 lowercase hex digits.")
    }
}

impl std::error::Error for InvalidRevision {}

impl FromStr for ManifestRevision {
    type Err = InvalidRevision;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let hex = text.strip_prefix("sha256:").ok_or(InvalidRevision)?;
        if hex.len() != 64 || !hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err(InvalidRevision);
        }
        let mut digest = [0; 32];
        for (index, byte) in digest.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[index * 2..][..2], 16).map_err(|_| InvalidRevision)?;
        }
        Ok(Self(digest))
    }
}

impl Serialize for ManifestRevision {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ManifestRevision {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(de::Error::custom)
    }
}
