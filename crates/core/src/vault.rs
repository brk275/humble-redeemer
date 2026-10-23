use anyhow::{Result, anyhow};
use chacha20poly1305::{
    ChaCha20Poly1305, Nonce,
    aead::{Aead, KeyInit},
};
use hmac::{Hmac, Mac};
use rand::{RngCore, rngs::OsRng};
use sha2::Sha256;
use zeroize::Zeroizing;

pub struct Vault {
    key: Zeroizing<[u8; 32]>,
    pub persistent: bool,
}
impl Vault {
    pub fn open(demo: bool) -> Self {
        if !demo
            && let Ok(entry) = keyring::Entry::new("io.github.brk275.humble-redeemer", "vault-v1")
        {
            match entry.get_secret() {
                Ok(bytes) if bytes.len() == 32 => {
                    let bytes = Zeroizing::new(bytes);
                    let mut key = [0; 32];
                    key.copy_from_slice(&bytes);
                    return Self {
                        key: Zeroizing::new(key),
                        persistent: true,
                    };
                }
                Err(keyring::Error::NoEntry) => {
                    let mut key = [0; 32];
                    OsRng.fill_bytes(&mut key);
                    if entry.set_secret(&key).is_ok() {
                        return Self {
                            key: Zeroizing::new(key),
                            persistent: true,
                        };
                    }
                }
                _ => {}
            }
        }
        Self::memory()
    }
    pub fn memory() -> Self {
        let mut key = [0; 32];
        OsRng.fill_bytes(&mut key);
        Self {
            key: Zeroizing::new(key),
            persistent: false,
        }
    }
    pub fn seal(&self, plain: &[u8]) -> Result<Vec<u8>> {
        let cipher = ChaCha20Poly1305::new_from_slice(self.key.as_ref())
            .map_err(|_| anyhow!("Vault unavailable"))?;
        let mut nonce = [0; 12];
        OsRng.fill_bytes(&mut nonce);
        let encrypted = cipher
            .encrypt(Nonce::from_slice(&nonce), plain)
            .map_err(|_| anyhow!("Encryption failed"))?;
        Ok([nonce.as_slice(), encrypted.as_slice()].concat())
    }
    pub fn unseal(&self, sealed: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        if sealed.len() < 28 {
            return Err(anyhow!("Invalid encrypted record"));
        }
        let cipher = ChaCha20Poly1305::new_from_slice(self.key.as_ref())
            .map_err(|_| anyhow!("Vault unavailable"))?;
        let plain = cipher
            .decrypt(Nonce::from_slice(&sealed[..12]), &sealed[12..])
            .map_err(|_| anyhow!("Unlock the original system keyring or reconnect this account"))?;
        Ok(Zeroizing::new(plain))
    }
    pub fn fingerprint(&self, bytes: &[u8]) -> String {
        let mut mac =
            <Hmac<Sha256> as Mac>::new_from_slice(self.key.as_ref()).expect("fixed key size");
        mac.update(bytes);
        hex::encode(mac.finalize().into_bytes())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encryption_roundtrip_and_tamper_detection() {
        let v = Vault::memory();
        let mut c = v.seal(b"fixture secret").unwrap();
        assert!(!String::from_utf8_lossy(&c).contains("fixture secret"));
        assert_eq!(&**v.unseal(&c).unwrap(), b"fixture secret");
        c[13] ^= 1;
        assert!(v.unseal(&c).is_err());
    }
    #[test]
    fn other_vault_cannot_decrypt() {
        let a = Vault::memory();
        let b = Vault::memory();
        assert!(b.unseal(&a.seal(b"fixture").unwrap()).is_err());
    }
}
