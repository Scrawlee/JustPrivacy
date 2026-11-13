use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chacha20poly1305::{aead::{Aead, KeyInit}, ChaCha20Poly1305, Nonce};
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::{fs, io::Write, path::Path};

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    rand::rngs::OsRng.fill_bytes(&mut b);
    b
}

/// Jeton opaque de 256 bits, encodé en base64url.
pub fn random_token() -> String {
    URL_SAFE_NO_PAD.encode(random_bytes::<32>())
}

pub fn sha256(data: &[u8]) -> Vec<u8> {
    Sha256::digest(data).to_vec()
}

/// Chiffrement au repos des secrets TOTP : nonce (12 octets) || texte chiffré.
pub struct Sealer(ChaCha20Poly1305);

impl Sealer {
    /// Clé dérivée de APP_SECRET, sinon lue (ou créée) dans DATA_DIR/app.key.
    pub fn load(app_secret: Option<&str>, data_dir: &Path) -> std::io::Result<Sealer> {
        let key: Vec<u8> = match app_secret {
            Some(s) => sha256(s.as_bytes()),
            None => {
                let path = data_dir.join("app.key");
                match fs::read_to_string(&path) {
                    Ok(hex_key) => hex::decode(hex_key.trim()).map_err(|e| std::io::Error::other(e))?,
                    Err(_) => {
                        fs::create_dir_all(data_dir)?;
                        let key = random_bytes::<32>();
                        let mut opts = fs::OpenOptions::new();
                        opts.write(true).create_new(true);
                        #[cfg(unix)]
                        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
                        opts.open(&path)?.write_all(hex::encode(key).as_bytes())?;
                        key.to_vec()
                    }
                }
            }
        };
        if key.len() != 32 {
            return Err(std::io::Error::other("clé de chiffrement invalide"));
        }
        Ok(Sealer(ChaCha20Poly1305::new(key.as_slice().into())))
    }

    pub fn seal(&self, plain: &[u8]) -> Vec<u8> {
        let nonce = random_bytes::<12>();
        let mut out = nonce.to_vec();
        out.extend(self.0.encrypt(Nonce::from_slice(&nonce), plain).expect("chiffrement"));
        out
    }

    pub fn open(&self, sealed: &[u8]) -> Option<Vec<u8>> {
        if sealed.len() < 13 { return None; }
        let (nonce, ct) = sealed.split_at(12);
        self.0.decrypt(Nonce::from_slice(nonce), ct).ok()
    }
}
