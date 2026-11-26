//! RFC 6238 TOTP (HMAC-SHA1, 30 s steps, 6 digits): what libpam-google-authenticator checks for
//! the fixture's `kbd` user.

use std::time::{SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use sha1::Sha1;

/// The `kbd` user's `~/.google_authenticator`, copied into the image.
const GOOGLE_AUTHENTICATOR: &str = include_str!("../../../tests/docker/google_authenticator");
/// Seconds per code.
pub const STEP: u64 = 30;
/// `" WINDOW_SIZE 3"` in the file: the server accepts the previous, current and next code.
const WINDOW: u64 = 1;

/// The base32 TOTP secret of user `kbd` (the first line of `tests/docker/google_authenticator`).
pub fn totp_secret() -> &'static str {
    GOOGLE_AUTHENTICATOR.lines().next().expect("secret line").trim()
}

/// The code for `unix_time` (seconds) with the fixture's secret.
pub fn totp_at(unix_time: u64) -> String {
    hotp(&base32_decode(totp_secret()), unix_time / STEP)
}

/// The code the server accepts now.
pub fn totp_now() -> String {
    totp_at(now())
}

/// A 6-digit code the server rejects now: none of the codes inside its window.
pub fn wrong_totp() -> String {
    let t = now();
    let accepted: Vec<String> =
        (0..=2 * WINDOW).map(|i| totp_at((t + i * STEP).saturating_sub(WINDOW * STEP))).collect();
    (0..10u32).map(|d| d.to_string().repeat(6)).find(|c| !accepted.contains(c)).expect("a rejected code")
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).expect("clock after 1970").as_secs()
}

/// RFC 4226 HOTP, 6 digits.
fn hotp(key: &[u8], counter: u64) -> String {
    let mut mac = Hmac::<Sha1>::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = usize::from(digest[19] & 0x0f);
    let word = u32::from_be_bytes(digest[offset..offset + 4].try_into().unwrap()) & 0x7fff_ffff;
    format!("{:06}", word % 1_000_000)
}

/// RFC 4648 base32 (upper case, padding optional).
fn base32_decode(s: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let (mut buf, mut bits) = (0u32, 0u32);
    for c in s.bytes().filter(|&c| c != b'=') {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'2'..=b'7' => c - b'2' + 26,
            _ => panic!("not base32: {s:?}"),
        };
        buf = (buf << 5) | u32::from(v);
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc_6238_sha1_vectors() {
        // RFC 6238 appendix B: the ASCII secret "12345678901234567890", 8 digits; the last 6
        // digits are the 6-digit code.
        let key = b"12345678901234567890";
        for (t, code) in
            [(59, "287082"), (1_111_111_109, "081804"), (1_234_567_890, "005924"), (2_000_000_000, "279037")]
        {
            assert_eq!(hotp(key, t / STEP), code, "t = {t}");
        }
    }

    #[test]
    fn base32_of_the_fixture_secret() {
        assert_eq!(base32_decode("JBSWY3DPEHPK3PXP"), b"Hello!\xde\xad\xbe\xef");
        assert_eq!(base32_decode(totp_secret()).len(), 10);
    }

    #[test]
    fn wrong_code_is_outside_the_window() {
        let wrong = wrong_totp();
        assert_eq!(wrong.len(), 6);
        assert_ne!(wrong, totp_now());
    }
}
