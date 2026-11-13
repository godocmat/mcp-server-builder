//! Read-only native filesystem access for an operator-selected root.
//!
//! Windows requires a real local NTFS drive mapping. Linux requires a trusted
//! mount namespace and a kernel with openat2 and procfs. The operator controls
//! the configured root and who can write to it.
//! Requested paths are limited to 4096 UTF-8 bytes and 64 components.

use std::{io, path::Path};

const MAX_PATH_BYTES: usize = 4096;
const MAX_COMPONENTS: usize = 64;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as platform;

#[cfg(not(any(windows, target_os = "linux")))]
mod platform {
    use std::{fs::File, io, path::Path};

    pub(super) struct Root;

    impl Root {
        pub(super) fn open(_path: &Path) -> io::Result<Self> {
            Err(io::ErrorKind::Unsupported.into())
        }

        pub(super) fn identity(&self) -> io::Result<[u8; 24]> {
            Err(io::ErrorKind::Unsupported.into())
        }

        pub(super) fn open_file(&self, _relative: &str) -> io::Result<File> {
            Err(io::ErrorKind::Unsupported.into())
        }
    }
}

pub(super) struct Root(platform::Root);

impl Root {
    pub(super) fn open(path: &Path) -> io::Result<Self> {
        platform::Root::open(path).map(Self)
    }

    pub(super) fn identity(&self) -> io::Result<[u8; 24]> {
        self.0.identity()
    }

    pub(super) fn open_file(&self, relative: &str) -> io::Result<impl io::Read + Send + '_> {
        validate_relative(relative)?;
        self.0.open_file(relative)
    }
}

fn validate_relative(relative: &str) -> io::Result<()> {
    if relative.len() > MAX_PATH_BYTES
        || relative.split('/').enumerate().any(|(index, name)| {
            index >= MAX_COMPONENTS || !valid_component(name) || name.contains(['{', '}'])
        })
    {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    Ok(())
}

fn valid_component(name: &str) -> bool {
    if name.is_empty()
        || name.ends_with(['.', ' '])
        || name
            .chars()
            .any(|ch| ch.is_control() || "/\\<>:\"|?*".contains(ch))
    {
        return false;
    }
    let stem = name
        .split_once('.')
        .map_or(name, |(stem, _)| stem)
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    !matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) && !stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"))
        .is_some_and(|number| {
            matches!(
                number,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
}

#[cfg(any(windows, target_os = "linux"))]
fn denied() -> io::Error {
    io::ErrorKind::PermissionDenied.into()
}

#[cfg(test)]
mod tests;
