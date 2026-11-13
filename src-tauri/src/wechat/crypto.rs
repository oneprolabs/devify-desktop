//! SQLCipher 4 page format used by WeChat 4.x (AES-256-CBC + HMAC-SHA512).
//!
//! Layout per 4096-byte page, offsets are half-open:
//! - page 1: salt [0,16), ciphertext [16,4016), iv [4016,4032), hmac [4032,4096)
//! - page n: ciphertext [0,4016), iv [4016,4032), hmac [4032,4096)
//!
//! HMAC input is `ciphertext || iv || pgno(u32 LE)`, keyed with
//! `PBKDF2-HMAC-SHA512(enc_key, salt ^ 0x3a, 2, 32)`.

use aes::Aes256;
use cbc::cipher::{block_padding::NoPadding, BlockDecryptMut, KeyIvInit};
use hmac::{Hmac, Mac};
use pbkdf2::pbkdf2_hmac;
use sha2::Sha512;

type Aes256CbcDec = cbc::Decryptor<Aes256>;

pub const PAGE_SIZE: usize = 4096;
pub const SALT_LEN: usize = 16;
pub const IV_LEN: usize = 16;
pub const HMAC_LEN: usize = 64;
pub const RESERVE_LEN: usize = IV_LEN + HMAC_LEN;
pub const KEY_LEN: usize = 32;
pub const SQLITE_HEADER: &[u8] = b"SQLite format 3\0";

/// WeChat 4.x derives each database key from a 32-byte account passphrase.
///
/// `PBKDF2-HMAC-SHA512(passphrase, salt, 256000, 32)`. The salt is the first 16 bytes of
/// the encrypted file, which is why one passphrase yields a different key per database.
pub const PASSPHRASE_ROUNDS: u32 = 256_000;

pub fn derive_enc_key(passphrase: &[u8], salt: &[u8]) -> Result<[u8; KEY_LEN], String> {
    if passphrase.is_empty() {
        return Err("口令为空，无法派生密钥。".into());
    }
    if salt.len() != SALT_LEN {
        return Err(format!("salt 长度应为 16 字节，实际 {}", salt.len()));
    }
    let mut out = [0u8; KEY_LEN];
    pbkdf2_hmac::<Sha512>(passphrase, salt, PASSPHRASE_ROUNDS, &mut out);
    Ok(out)
}

/// Derive the per-database key from a passphrase, then verify it against page 1.
pub fn verify_passphrase(page1: &[u8], passphrase: &[u8]) -> Option<[u8; KEY_LEN]> {
    let salt = read_salt(page1)?;
    let key = derive_enc_key(passphrase, &salt).ok()?;
    if verify_key(page1, &key) {
        Some(key)
    } else {
        None
    }
}

/// Derive the per-database HMAC key from the raw encryption key and page-1 salt.
pub fn derive_mac_key(enc_key: &[u8], salt: &[u8]) -> Result<[u8; KEY_LEN], String> {
    if enc_key.len() != KEY_LEN {
        return Err(format!("密钥长度应为 32 字节，实际 {}", enc_key.len()));
    }
    if salt.len() != SALT_LEN {
        return Err(format!("salt 长度应为 16 字节，实际 {}", salt.len()));
    }
    let mac_salt: Vec<u8> = salt.iter().map(|b| b ^ 0x3a).collect();
    let mut out = [0u8; KEY_LEN];
    pbkdf2_hmac::<Sha512>(enc_key, &mac_salt, 2, &mut out);
    Ok(out)
}

/// Verify the stored HMAC of one encrypted page.
pub fn verify_page(page: &[u8], mac_key: &[u8], pgno: u32) -> bool {
    if page.len() < PAGE_SIZE || mac_key.len() != KEY_LEN {
        return false;
    }
    let start = if pgno == 1 { SALT_LEN } else { 0 };
    let mut mac = match Hmac::<Sha512>::new_from_slice(mac_key) {
        Ok(mac) => mac,
        Err(_) => return false,
    };
    mac.update(&page[start..PAGE_SIZE - RESERVE_LEN + IV_LEN]);
    mac.update(&pgno.to_le_bytes());
    let stored = &page[PAGE_SIZE - HMAC_LEN..PAGE_SIZE];
    mac.verify_slice(stored).is_ok()
}

/// Decrypt one page into a full 4096-byte plaintext page.
pub fn decrypt_page(page: &[u8], enc_key: &[u8], pgno: u32) -> Result<Vec<u8>, String> {
    if page.len() < PAGE_SIZE {
        return Err(format!("第 {pgno} 页长度不足 4096 字节"));
    }
    let iv = &page[PAGE_SIZE - RESERVE_LEN..PAGE_SIZE - RESERVE_LEN + IV_LEN];
    let cipher = Aes256CbcDec::new_from_slices(enc_key, iv)
        .map_err(|_| "AES 初始化失败：密钥或 IV 长度不合法".to_string())?;

    let mut out = vec![0u8; PAGE_SIZE];
    if pgno == 1 {
        let ciphertext = &page[SALT_LEN..PAGE_SIZE - RESERVE_LEN];
        let plain = cipher
            .decrypt_padded_vec_mut::<NoPadding>(ciphertext)
            .map_err(|_| "第 1 页解密失败".to_string())?;
        // Plaintext occupies [16, 4016); the header replaces the salt area and the
        // reserve tail is zeroed, matching what SQLCipher itself hands to SQLite.
        out[..SQLITE_HEADER.len()].copy_from_slice(SQLITE_HEADER);
        out[SQLITE_HEADER.len()..PAGE_SIZE - RESERVE_LEN].copy_from_slice(&plain);
    } else {
        let ciphertext = &page[..PAGE_SIZE - RESERVE_LEN];
        let plain = cipher
            .decrypt_padded_vec_mut::<NoPadding>(ciphertext)
            .map_err(|_| format!("第 {pgno} 页解密失败"))?;
        out[..PAGE_SIZE - RESERVE_LEN].copy_from_slice(&plain);
        for byte in &mut out[PAGE_SIZE - RESERVE_LEN..] {
            *byte = 0;
        }
    }
    Ok(out)
}

/// Read the 16-byte salt of an encrypted database, or `None` for plaintext SQLite.
pub fn read_salt(header: &[u8]) -> Option<[u8; SALT_LEN]> {
    if header.len() < SALT_LEN || header.starts_with(SQLITE_HEADER) {
        return None;
    }
    let mut salt = [0u8; SALT_LEN];
    salt.copy_from_slice(&header[..SALT_LEN]);
    Some(salt)
}

/// Verify that a candidate key decrypts page 1 of the given database bytes.
pub fn verify_key(page1: &[u8], enc_key: &[u8]) -> bool {
    let salt = match read_salt(page1) {
        Some(salt) => salt,
        None => return false,
    };
    match derive_mac_key(enc_key, &salt) {
        Ok(mac_key) => verify_page(page1, &mac_key, 1),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_key_is_deterministic_and_key_sized() {
        let key = [7u8; KEY_LEN];
        let salt = [9u8; SALT_LEN];
        let a = derive_mac_key(&key, &salt).unwrap();
        let b = derive_mac_key(&key, &salt).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.len(), KEY_LEN);
    }

    #[test]
    fn plaintext_header_has_no_salt() {
        assert!(read_salt(SQLITE_HEADER).is_none());
        assert!(read_salt(&[0u8; 16]).is_some());
    }

    #[test]
    fn passphrase_derivation_is_deterministic_and_salt_scoped() {
        let passphrase = [0x24u8; KEY_LEN];
        let salt_a = [1u8; SALT_LEN];
        let salt_b = [2u8; SALT_LEN];
        let a = derive_enc_key(&passphrase, &salt_a).unwrap();
        assert_eq!(a, derive_enc_key(&passphrase, &salt_a).unwrap());
        assert_ne!(a, derive_enc_key(&passphrase, &salt_b).unwrap());
        assert_eq!(a.len(), KEY_LEN);
    }

    #[test]
    fn passphrase_derivation_rejects_bad_lengths() {
        assert!(derive_enc_key(&[], &[0u8; SALT_LEN]).is_err());
        assert!(derive_enc_key(&[1u8; KEY_LEN], &[0u8; 4]).is_err());
    }
}
