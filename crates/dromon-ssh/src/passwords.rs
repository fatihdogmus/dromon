//! Passwords and key passphrases, in memory only, so silent reconnects don't ask again.
//!
//! Owned by the service thread. Secrets are `SecretString`s (zeroized on drop); this type has no
//! `Serialize`, and its `Debug` prints only counts.

use std::collections::HashMap;
use std::fmt;

use dromon_core::HostId;
use secrecy::SecretString;

/// Passwords and key passphrases are kept apart: passwords by host and identity (the
/// `user@host` the prompt names, see [`crate::password_identity`], so a ProxyJump's jump host and
/// target each keep their own), passphrases by host and key path (`None` when ssh didn't name
/// the key), so a passphrase never answers a password prompt and two keys never share one.
#[derive(Default)]
pub struct PasswordStore {
    passwords: HashMap<(HostId, String), SecretString>,
    passphrases: HashMap<(HostId, Option<String>), SecretString>,
}

impl fmt::Debug for PasswordStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PasswordStore")
            .field("passwords", &self.passwords.len())
            .field("passphrases", &self.passphrases.len())
            .finish()
    }
}

impl PasswordStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// The remembered password for `identity` (e.g. `deploy@10.0.0.4`) on `host`.
    pub fn get(&self, host: &HostId, identity: &str) -> Option<&SecretString> {
        self.passwords.get(&(host.clone(), identity.to_owned()))
    }

    pub fn set(&mut self, host: HostId, identity: String, password: SecretString) {
        self.passwords.insert((host, identity), password);
    }

    /// Forgets the password for `identity` on `host` (it was rejected). Other identities' passwords
    /// and all passphrases stay.
    pub fn clear_password(&mut self, host: &HostId, identity: &str) {
        self.passwords.remove(&(host.clone(), identity.to_owned()));
    }

    /// The remembered passphrase for the key at `key_path` on `host`.
    pub fn passphrase(&self, host: &HostId, key_path: Option<&str>) -> Option<&SecretString> {
        self.passphrases.get(&(host.clone(), key_path.map(str::to_owned)))
    }

    pub fn set_passphrase(&mut self, host: HostId, key_path: Option<String>, passphrase: SecretString) {
        self.passphrases.insert((host, key_path), passphrase);
    }

    /// Forgets one key's passphrase (it was rejected).
    pub fn clear_passphrase(&mut self, host: &HostId, key_path: Option<&str>) {
        self.passphrases.remove(&(host.clone(), key_path.map(str::to_owned)));
    }

    /// Forgets everything remembered for `host`: all its passwords and passphrases. Use on
    /// Disconnect.
    pub fn clear(&mut self, host: &HostId) {
        self.passwords.retain(|(h, _), _| h != host);
        self.passphrases.retain(|(h, _), _| h != host);
    }

    /// Forgets everything. Use on quit.
    pub fn clear_all(&mut self) {
        self.passwords.clear();
        self.passphrases.clear();
    }

    /// Whether nothing is remembered.
    pub fn is_empty(&self) -> bool {
        self.passwords.is_empty() && self.passphrases.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use secrecy::ExposeSecret;

    use super::*;

    fn host(s: &str) -> HostId {
        HostId(s.into())
    }

    const ME: &str = "deploy@10.0.0.4";
    const JUMP: &str = "jump@bastion";

    #[test]
    fn set_get_clear() {
        let mut s = PasswordStore::new();
        assert!(s.is_empty());
        s.set(host("a"), ME.into(), "pw-a".into());
        s.set(host("b"), ME.into(), "pw-b".into());
        assert_eq!(s.get(&host("a"), ME).unwrap().expose_secret(), "pw-a");
        s.set(host("a"), ME.into(), "pw-a2".into());
        assert_eq!(s.get(&host("a"), ME).unwrap().expose_secret(), "pw-a2");
        s.clear_password(&host("a"), ME);
        assert!(s.get(&host("a"), ME).is_none());
        assert!(s.get(&host("b"), ME).is_some());
        s.clear_all();
        assert!(s.is_empty());
    }

    #[test]
    fn passwords_are_per_identity_within_a_host() {
        let mut s = PasswordStore::new();
        s.set(host("a"), ME.into(), "target".into());
        s.set(host("a"), JUMP.into(), "jump".into());
        assert_eq!(s.get(&host("a"), ME).unwrap().expose_secret(), "target");
        assert_eq!(s.get(&host("a"), JUMP).unwrap().expose_secret(), "jump");
        assert!(s.get(&host("a"), "other@x").is_none());
        s.clear_password(&host("a"), JUMP);
        assert!(s.get(&host("a"), JUMP).is_none());
        assert_eq!(s.get(&host("a"), ME).unwrap().expose_secret(), "target");
    }

    #[test]
    fn passphrases_are_separate_from_passwords_and_per_key() {
        let mut s = PasswordStore::new();
        s.set(host("a"), ME.into(), "pw".into());
        s.set_passphrase(host("a"), Some("/k1".into()), "pp1".into());
        s.set_passphrase(host("a"), None, "pp-unnamed".into());
        assert_eq!(s.passphrase(&host("a"), Some("/k1")).unwrap().expose_secret(), "pp1");
        assert_eq!(s.passphrase(&host("a"), None).unwrap().expose_secret(), "pp-unnamed");
        assert!(s.passphrase(&host("a"), Some("/k2")).is_none());
        assert!(s.passphrase(&host("b"), Some("/k1")).is_none());
        assert_eq!(s.get(&host("a"), ME).unwrap().expose_secret(), "pw");

        s.clear_passphrase(&host("a"), Some("/k1"));
        assert!(s.passphrase(&host("a"), Some("/k1")).is_none());
        assert!(s.get(&host("a"), ME).is_some());
        s.clear_password(&host("a"), ME);
        assert!(s.passphrase(&host("a"), None).is_some());
    }

    #[test]
    fn clear_forgets_everything_for_one_host() {
        let mut s = PasswordStore::new();
        s.set(host("a"), ME.into(), "pw".into());
        s.set(host("a"), JUMP.into(), "pw-jump".into());
        s.set_passphrase(host("a"), Some("/k".into()), "pp".into());
        s.set(host("b"), ME.into(), "pw-b".into());
        s.set_passphrase(host("b"), Some("/k".into()), "pp-b".into());
        s.clear(&host("a"));
        assert!(s.get(&host("a"), ME).is_none());
        assert!(s.get(&host("a"), JUMP).is_none());
        assert!(s.passphrase(&host("a"), Some("/k")).is_none());
        assert!(s.get(&host("b"), ME).is_some());
        assert!(s.passphrase(&host("b"), Some("/k")).is_some());
    }

    #[test]
    fn debug_prints_no_secrets_or_hosts() {
        let mut s = PasswordStore::new();
        s.set(host("secret-host"), "secret-user@secret-host".into(), "hunter2".into());
        s.set_passphrase(host("secret-host"), Some("/k".into()), "correct horse".into());
        let dbg = format!("{s:?}");
        assert!(!dbg.contains("hunter2") && !dbg.contains("correct horse") && !dbg.contains("secret-"));
        assert_eq!(dbg, "PasswordStore { passwords: 1, passphrases: 1 }");
    }
}

#[cfg(test)]
#[path = "passwords_props.rs"]
mod props;
