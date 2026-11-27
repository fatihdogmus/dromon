//! What an update must prove: the release key built in, signatures over `SHA256SUMS` (the
//! test-only key's, `update_testdata/`, never the release key's), and the file against the sums.

use super::*;

/// The test-only fixtures (`update_testdata/make.py`).
pub(crate) const TEST_KEY: &str = include_str!("update_testdata/test-only-key.pub");
pub(crate) const SUMS: &[u8] = include_bytes!("update_testdata/SHA256SUMS");
pub(crate) const SIGNATURE: &str = include_str!("update_testdata/SHA256SUMS.minisig");
const OTHER_KEY: &str = include_str!("update_testdata/SHA256SUMS.other-key.minisig");
const OTHER_RELEASE: &str = include_str!("update_testdata/SHA256SUMS.other-release.minisig");
const LEGACY: &str = include_str!("update_testdata/SHA256SUMS.legacy.minisig");
pub(crate) const DMG: &[u8] = include_bytes!("update_testdata/Dromon-9.9.9-macos-universal.dmg");
pub(crate) const DMG_NAME: &str = "Dromon-9.9.9-macos-universal.dmg";

fn key() -> PublicKey {
    parse_key(TEST_KEY).expect("the test key parses")
}

fn version() -> Version {
    Version::parse("9.9.9").unwrap()
}

fn sha256(bytes: &[u8]) -> Vec<u8> {
    ring::digest::digest(&ring::digest::SHA256, bytes).as_ref().to_vec()
}

/// Standard base64 (with padding), for reading a key's id here.
fn base64(text: &str) -> Vec<u8> {
    const ABC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0);
    for c in text.bytes().filter(|&c| c != b'=') {
        let value = ABC.iter().position(|&a| a == c).expect("base64") as u32;
        acc = (acc << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

#[test]
fn the_shipped_release_key_is_minisign_key_489cb3c37a12ec48() {
    release_key().expect("crates/dromon-app/assets/release-key.pub is a minisign public key");
    let line = RELEASE_KEY.lines().nth(1).expect("the key's line");
    let bin = base64(line);
    assert_eq!((bin.len(), &bin[..2]), (42, &b"Ed"[..]), "an Ed25519 minisign key");
    let id = u64::from_le_bytes(bin[2..10].try_into().unwrap());
    assert_eq!(format!("{id:016X}"), "489CB3C37A12EC48");
    assert!(RELEASE_KEY.starts_with("untrusted comment: minisign public key 489CB3C37A12EC48\n"));
}

#[test]
fn a_missing_or_malformed_key_is_no_key() {
    for text in ["", "untrusted comment: PLACEHOLDER\nNOT-A-KEY\n", "RWQ=", "untrusted comment: only a comment"] {
        assert_eq!(parse_key(text).unwrap_err(), "This build of Dromon has no usable release key.", "{text:?}");
    }
}

#[test]
fn the_release_signature_and_its_file_pass() {
    assert_eq!(signed_sums(&key(), &version(), SUMS, SIGNATURE), Ok(()));
    assert_eq!(listed(SUMS, DMG_NAME, &sha256(DMG)), Ok(()));
}

#[test]
fn tampered_sums_refuse() {
    let mut sums = SUMS.to_vec();
    // One hex digit of the DMG's line changed, as an attacker pointing it at their file would.
    sums[0] = if sums[0] == b'0' { b'1' } else { b'0' };
    assert_eq!(signed_sums(&key(), &version(), &sums, SIGNATURE), Err(Refused::Signature));
    // A line added for another file.
    let mut more = SUMS.to_vec();
    more.extend_from_slice(b"0000000000000000000000000000000000000000000000000000000000000000  evil.dmg\n");
    assert_eq!(signed_sums(&key(), &version(), &more, SIGNATURE), Err(Refused::Signature));
}

#[test]
fn another_keys_signature_refuses() {
    assert_eq!(signed_sums(&key(), &version(), SUMS, OTHER_KEY), Err(Refused::Signature));
}

#[test]
fn a_signature_for_another_release_refuses() {
    assert_eq!(signed_sums(&key(), &version(), SUMS, OTHER_RELEASE), Err(Refused::OtherRelease));
    // And the right one for 9.9.9 doesn't pass for 9.9.8.
    let older = Version::parse("9.9.8").unwrap();
    assert_eq!(signed_sums(&key(), &older, SUMS, SIGNATURE), Err(Refused::OtherRelease));
}

#[test]
fn legacy_and_malformed_signatures_refuse() {
    assert_eq!(signed_sums(&key(), &version(), SUMS, LEGACY), Err(Refused::Signature));
    for bad in ["", "untrusted comment: x\n", "untrusted comment: x\nnot base64\ntrusted comment: x\nnope\n"] {
        assert_eq!(signed_sums(&key(), &version(), SUMS, bad), Err(Refused::Signature), "{bad:?}");
    }
    // The global signature over the trusted comment: a comment edited by hand breaks it.
    let edited = SIGNATURE.replace("trusted comment: dromon 9.9.9", "trusted comment: dromon 9.9.9 ");
    assert_eq!(signed_sums(&key(), &version(), SUMS, &edited), Err(Refused::Signature));
}

#[test]
fn a_tampered_file_refuses() {
    let mut dmg = DMG.to_vec();
    dmg[0] ^= 1;
    assert_eq!(listed(SUMS, DMG_NAME, &sha256(&dmg)), Err(Refused::Mismatch));
    // The MSI's line doesn't vouch for the DMG.
    let msi = include_bytes!("update_testdata/Dromon-9.9.9-windows-x64.msi");
    assert_eq!(listed(SUMS, DMG_NAME, &sha256(msi)), Err(Refused::Mismatch));
}

#[test]
fn the_file_must_be_listed_exactly_once() {
    let digest = sha256(DMG);
    assert_eq!(listed(SUMS, "Dromon-9.9.9-linux-x64.deb", &digest), Err(Refused::NotListed));
    assert_eq!(listed(SUMS, "macos-universal.dmg", &digest), Err(Refused::NotListed), "whole names only");
    let twice = [SUMS, SUMS].concat();
    assert_eq!(listed(&twice, DMG_NAME, &digest), Err(Refused::NotListed));
    assert_eq!(listed(&[0xff, 0xfe], DMG_NAME, &digest), Err(Refused::NotListed));
    // sha256sum's binary mode and upper-case digits read the same.
    let binary = format!("{} *{DMG_NAME}\n", to_hex(&digest).to_uppercase());
    assert_eq!(listed(binary.as_bytes(), DMG_NAME, &digest), Ok(()));
}

#[test]
fn refusals_read_as_sentences() {
    assert_eq!(
        Refused::Mismatch.to_string(),
        "the download doesn't match the release's checksums, so it wasn't installed"
    );
    assert_eq!(trusted_comment(&version()), "dromon 9.9.9");
}
