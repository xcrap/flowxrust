//! Who may ask the app, and for which thread.
//!
//! The app makes one random secret per launch and never writes it down. A
//! thread's token is an HMAC of the thread's id under that secret, so:
//!
//! - **a token names its thread.** The listener recomputes it from the thread
//!   a request claims; a token handed to one thread's server cannot speak for
//!   another thread, even in the same project.
//! - **the app keeps no table of tokens.** There is nothing to grow, expire or
//!   race; a relaunch makes a new secret, and every token from before it stops
//!   working.
//! - **a token is compared in constant time**, so how long a refusal takes
//!   says nothing about how much of a guess was right.

use std::io::Read;

use sha2::{Digest, Sha256};

/// What every thread token is derived from. Never logged, never saved.
pub struct Secret([u8; 32]);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(…)")
    }
}

/// What the token is bound to besides the thread, so a value computed for
/// anything else under the same key cannot pass as one.
const CONTEXT: &[u8] = b"flowx-mcp-thread:";

impl Secret {
    /// A fresh secret from the system's random source.
    pub fn generate() -> std::io::Result<Self> {
        let mut bytes = [0u8; 32];
        std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
        Ok(Self(bytes))
    }

    /// The token a thread's server presents.
    pub fn token_for(&self, thread: &str) -> String {
        let mut message = CONTEXT.to_vec();
        message.extend_from_slice(thread.as_bytes());
        hex(&hmac_sha256(&self.0, &message))
    }

    /// Whether `presented` is that thread's token.
    pub fn verify(&self, thread: &str, presented: &str) -> bool {
        constant_time_eq(self.token_for(thread).as_bytes(), presented.as_bytes())
    }
}

/// HMAC-SHA256 (RFC 2104), over the SHA-256 the workspace already has.
fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut block = [0u8; BLOCK];
    if key.len() > BLOCK {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let inner_key: Vec<u8> = block.iter().map(|byte| byte ^ 0x36).collect();
    let outer_key: Vec<u8> = block.iter().map(|byte| byte ^ 0x5c).collect();
    let inner = Sha256::new()
        .chain_update(&inner_key)
        .chain_update(message)
        .finalize();
    Sha256::new()
        .chain_update(&outer_key)
        .chain_update(inner)
        .finalize()
        .into()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Equal, in a time that depends only on the lengths.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mac_is_hmac_sha256() {
        // RFC 4231, test cases 2 and 6 (a key longer than the block).
        assert_eq!(
            hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        assert_eq!(
            hex(&hmac_sha256(
                &[0xaa; 131],
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            )),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn a_token_speaks_for_its_own_thread_and_no_other() {
        let secret = Secret::generate().unwrap();
        let token = secret.token_for("thread-a");
        assert_eq!(token.len(), 64);
        assert!(secret.verify("thread-a", &token));
        assert!(!secret.verify("thread-b", &token));
        assert!(!secret.verify("thread-a", &token[..63]));
        assert!(!secret.verify("thread-a", ""));
    }

    #[test]
    fn a_new_launch_makes_every_old_token_stale() {
        let before = Secret::generate().unwrap();
        let after = Secret::generate().unwrap();
        assert!(!after.verify("thread-a", &before.token_for("thread-a")));
    }

    #[test]
    fn the_secret_never_prints() {
        let secret = Secret([7; 32]);
        assert_eq!(format!("{secret:?}"), "Secret(…)");
    }
}
