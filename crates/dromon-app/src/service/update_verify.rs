//! What an update must prove before anything installs it (part of the release signing
//! boundary, docs/security.md):
//!
//! 1. `SHA256SUMS.minisig` is a minisign signature of the release's `SHA256SUMS` by the release
//!    key built into the app ([`release_key`], `crates/dromon-app/assets/release-key.pub`), with the
//!    trusted comment `dromon <version>` the release pipeline signs it with
//!    (`.github/release/sign-minisign.sh`), so another release's signed sums don't pass for
//!    this one's.
//! 2. The downloaded file's SHA-256 is the one `SHA256SUMS` lists for its name, exactly once.
//!
//! The signature is checked first, before the large download starts. Anything else refuses the
//! update with a [`Refused`] the user reads; nothing is installed and nothing changes.

use std::fmt;

use minisign_verify::{PublicKey, Signature};

use crate::state::update::Version;

/// Also the test-only fixtures the updater's tests download (`update_tests.rs`).
#[cfg(test)]
#[path = "update_verify_tests.rs"]
pub(super) mod tests;

/// The release key's public half, as `minisign -G` wrote it. It lives in this crate because
/// `cargo dev` builds a copy of the crates alone.
const RELEASE_KEY: &str = include_str!("../../assets/release-key.pub");

/// The release key the app trusts, or why this build has none it can use (the file is missing
/// its key or isn't a minisign public key): then updates are found but never installed.
pub fn release_key() -> Result<PublicKey, String> {
    parse_key(RELEASE_KEY)
}

/// A minisign public key file's key.
pub fn parse_key(text: &str) -> Result<PublicKey, String> {
    PublicKey::decode(text.trim()).map_err(|_| "This build of Dromon has no usable release key.".to_owned())
}

/// Why an update was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The signature is malformed, or not the release key's over these sums.
    Signature,
    /// Signed, but for another release.
    OtherRelease,
    /// `SHA256SUMS` doesn't list the file exactly once.
    NotListed,
    /// The file isn't the one `SHA256SUMS` lists: it changed on its way.
    Mismatch,
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Refused::Signature => "its checksums aren't signed with Dromon's release key, so it wasn't installed",
            Refused::OtherRelease => "its signature is for another release, so it wasn't installed",
            Refused::NotListed => "the release's checksums don't list it, so it wasn't installed",
            Refused::Mismatch => "the download doesn't match the release's checksums, so it wasn't installed",
        })
    }
}

/// The trusted comment a release's signature carries.
pub fn trusted_comment(version: &Version) -> String {
    format!("dromon {version}")
}

/// Step 1: `signature` is the release key's over `sums`, for `version`.
pub fn signed_sums(key: &PublicKey, version: &Version, sums: &[u8], signature: &str) -> Result<(), Refused> {
    let signature = Signature::decode(signature).map_err(|_| Refused::Signature)?;
    // Pre-hashed signatures only (minisign's default since 0.8): the legacy kind never verifies.
    key.verify(sums, &signature, false).map_err(|_| Refused::Signature)?;
    if signature.trusted_comment() != trusted_comment(version) {
        return Err(Refused::OtherRelease);
    }
    Ok(())
}

/// Step 2: `sums` (sha256sum's format) lists `name` once, with the SHA-256 `digest`.
pub fn listed(sums: &[u8], name: &str, digest: &[u8]) -> Result<(), Refused> {
    let text = std::str::from_utf8(sums).map_err(|_| Refused::NotListed)?;
    let mut lines = text.lines().filter_map(|line| {
        let (hex, rest) = line.split_once(' ')?;
        // `<hex>  <name>` (text mode) or `<hex> *<name>` (binary mode).
        let file = rest.strip_prefix(' ').or_else(|| rest.strip_prefix('*'))?;
        (file == name).then_some(hex)
    });
    let (Some(hex), None) = (lines.next(), lines.next()) else { return Err(Refused::NotListed) };
    if hex.eq_ignore_ascii_case(&to_hex(digest)) { Ok(()) } else { Err(Refused::Mismatch) }
}

/// Lower-case hex.
pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
