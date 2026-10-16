//! WeCom (企业微信) callback crypto: SHA1 signature + AES-256-CBC (WXBizMsgCrypt).
//!
//! EncodingAESKey is 43 Base64 characters (32-byte key after decoding with a trailing `=`).
//! Padding follows Tencent’s 32-byte PKCS7 variant; AES itself still uses 16-byte blocks.

use aes::Aes256;
use base64::Engine;
use cbc::{Decryptor, Encryptor};
use cipher::{block_padding::NoPadding, BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use sha1::{Digest, Sha1};

type Aes256CbcEnc = Encryptor<Aes256>;
type Aes256CbcDec = Decryptor<Aes256>;

const PKCS7_BLOCK: usize = 32;
const RANDOM_LEN: usize = 16;

#[derive(Debug, Clone)]
pub struct WxBizMsgCrypt {
    token: String,
    aes_key: [u8; 32],
    receive_id: String,
}

impl WxBizMsgCrypt {
    pub fn new(token: &str, encoding_aes_key: &str, receive_id: &str) -> Result<Self, String> {
        let token = token.trim();
        let encoding_aes_key = encoding_aes_key.trim();
        let receive_id = receive_id.trim();
        if token.is_empty() {
            return Err("WeCom callback token is empty".into());
        }
        if receive_id.is_empty() {
            return Err("WeCom corp id is empty".into());
        }
        let aes_key = decode_encoding_aes_key(encoding_aes_key)?;
        Ok(Self {
            token: token.to_string(),
            aes_key,
            receive_id: receive_id.to_string(),
        })
    }

    pub fn signature(&self, timestamp: &str, nonce: &str, encrypt: &str) -> String {
        sha1_signature(&self.token, timestamp, nonce, encrypt)
    }

    pub fn verify_signature(
        &self,
        msg_signature: &str,
        timestamp: &str,
        nonce: &str,
        encrypt: &str,
    ) -> bool {
        signatures_equal(&self.signature(timestamp, nonce, encrypt), msg_signature)
    }

    /// Decrypt a Base64 ciphertext (echostr or POST Encrypt) after verifying `msg_signature`.
    pub fn verify_and_decrypt(
        &self,
        msg_signature: &str,
        timestamp: &str,
        nonce: &str,
        encrypt_b64: &str,
    ) -> Result<String, String> {
        if !self.verify_signature(msg_signature, timestamp, nonce, encrypt_b64) {
            return Err("WeCom msg_signature mismatch".into());
        }
        self.decrypt(encrypt_b64)
    }

    pub fn decrypt(&self, encrypt_b64: &str) -> Result<String, String> {
        let ciphertext = base64::engine::general_purpose::STANDARD
            .decode(encrypt_b64.trim())
            .map_err(|e| format!("WeCom encrypt is not valid Base64: {e}"))?;
        if ciphertext.len() < 32 || ciphertext.len() % 16 != 0 {
            return Err("WeCom ciphertext length is invalid".into());
        }
        let mut buf = ciphertext;
        let iv: [u8; 16] = self.aes_key[..16].try_into().expect("AES key is 32 bytes");
        let decrypted = Aes256CbcDec::new((&self.aes_key).into(), &iv.into())
            .decrypt_padded_mut::<NoPadding>(&mut buf)
            .map_err(|e| format!("WeCom AES decrypt failed: {e}"))?;
        let unpadded = pkcs7_unpad(decrypted)?;
        if unpadded.len() < RANDOM_LEN + 4 {
            return Err("WeCom plaintext is too short".into());
        }
        let msg_len = u32::from_be_bytes(
            unpadded[RANDOM_LEN..RANDOM_LEN + 4]
                .try_into()
                .map_err(|_| "WeCom plaintext length prefix is truncated")?,
        ) as usize;
        let msg_start = RANDOM_LEN + 4;
        let msg_end = msg_start
            .checked_add(msg_len)
            .ok_or_else(|| "WeCom plaintext length overflows".to_string())?;
        if msg_end > unpadded.len() {
            return Err("WeCom plaintext length exceeds buffer".into());
        }
        let xml = std::str::from_utf8(&unpadded[msg_start..msg_end])
            .map_err(|e| format!("WeCom plaintext is not UTF-8: {e}"))?;
        let tail = std::str::from_utf8(&unpadded[msg_end..])
            .map_err(|e| format!("WeCom receive id tail is not UTF-8: {e}"))?;
        if tail != self.receive_id {
            return Err("WeCom decrypt receive id mismatch".into());
        }
        Ok(xml.to_string())
    }

    /// Encrypt plaintext XML (used in tests and optional encrypted replies).
    pub fn encrypt(&self, plaintext_xml: &str) -> Result<String, String> {
        let mut packed =
            Vec::with_capacity(RANDOM_LEN + 4 + plaintext_xml.len() + self.receive_id.len());
        packed.extend_from_slice(b"0123456789abcdef");
        let msg = plaintext_xml.as_bytes();
        packed.extend_from_slice(&(msg.len() as u32).to_be_bytes());
        packed.extend_from_slice(msg);
        packed.extend_from_slice(self.receive_id.as_bytes());
        let padded = pkcs7_pad(&packed);
        let iv: [u8; 16] = self.aes_key[..16].try_into().expect("AES key is 32 bytes");
        let mut buf = padded;
        let data_len = buf.len();
        let encrypted = Aes256CbcEnc::new((&self.aes_key).into(), &iv.into())
            .encrypt_padded_mut::<NoPadding>(&mut buf, data_len)
            .map_err(|e| format!("WeCom AES encrypt failed: {e}"))?;
        Ok(base64::engine::general_purpose::STANDARD.encode(encrypted))
    }
}

pub fn sha1_signature(token: &str, timestamp: &str, nonce: &str, encrypt: &str) -> String {
    let mut parts = [token, timestamp, nonce, encrypt];
    parts.sort_unstable();
    let mut hasher = Sha1::new();
    for part in parts {
        hasher.update(part.as_bytes());
    }
    hex_encode(&hasher.finalize())
}

fn signatures_equal(expected: &str, provided: &str) -> bool {
    expected.eq_ignore_ascii_case(provided.trim())
}

/// AES-256-CBC with WeCom's 32-byte PKCS7 padding (AI Bot media `aeskey`).
pub fn decrypt_aes256_cbc_pkcs7_32(key: &[u8; 32], ciphertext: &[u8]) -> Result<Vec<u8>, String> {
    if ciphertext.len() < 32 || ciphertext.len() % 16 != 0 {
        return Err("WeCom media ciphertext length is invalid".into());
    }
    let mut buf = ciphertext.to_vec();
    let iv: [u8; 16] = key[..16].try_into().expect("AES key is 32 bytes");
    let decrypted = Aes256CbcDec::new(key.into(), &iv.into())
        .decrypt_padded_mut::<NoPadding>(&mut buf)
        .map_err(|e| format!("WeCom media AES decrypt failed: {e}"))?;
    Ok(pkcs7_unpad(decrypted)?.to_vec())
}

pub fn parse_media_aeskey(raw: &str) -> Result<[u8; 32], String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("WeCom media aeskey is empty".into());
    }
    if raw.len() == 43 {
        return decode_encoding_aes_key(raw);
    }
    if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(raw.as_bytes()) {
        if let Ok(key) = <[u8; 32]>::try_from(decoded.as_slice()) {
            return Ok(key);
        }
    }
    if raw.len() == 64 {
        let mut out = [0u8; 32];
        for i in 0..32 {
            out[i] = u8::from_str_radix(&raw[i * 2..i * 2 + 2], 16)
                .map_err(|_| "WeCom media aeskey is not valid hex".to_string())?;
        }
        return Ok(out);
    }
    Err("WeCom media aeskey must be 32-byte Base64 or 64-char hex".into())
}

fn decode_encoding_aes_key(encoding_aes_key: &str) -> Result<[u8; 32], String> {
    if encoding_aes_key.len() != 43 {
        return Err(format!(
            "WeCom EncodingAESKey must be 43 characters (got {})",
            encoding_aes_key.len()
        ));
    }
    let mut padded = String::with_capacity(44);
    padded.push_str(encoding_aes_key);
    padded.push('=');
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(padded.as_bytes())
        .map_err(|e| format!("WeCom EncodingAESKey is not valid Base64: {e}"))?;
    decoded.try_into().map_err(|v: Vec<u8>| {
        format!(
            "WeCom EncodingAESKey must decode to 32 bytes (got {})",
            v.len()
        )
    })
}

fn pkcs7_pad(data: &[u8]) -> Vec<u8> {
    let mut amount = PKCS7_BLOCK - (data.len() % PKCS7_BLOCK);
    if amount == 0 {
        amount = PKCS7_BLOCK;
    }
    let mut out = Vec::with_capacity(data.len() + amount);
    out.extend_from_slice(data);
    out.extend(std::iter::repeat_n(amount as u8, amount));
    out
}

fn pkcs7_unpad(data: &[u8]) -> Result<&[u8], String> {
    if data.is_empty() {
        return Err("WeCom PKCS7 payload is empty".into());
    }
    let pad = data[data.len() - 1] as usize;
    if !(1..=PKCS7_BLOCK).contains(&pad) || pad > data.len() {
        return Err("WeCom PKCS7 padding is invalid".into());
    }
    if !data[data.len() - pad..].iter().all(|b| *b as usize == pad) {
        return Err("WeCom PKCS7 padding bytes are inconsistent".into());
    }
    Ok(&data[..data.len() - pad])
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_crypt() -> WxBizMsgCrypt {
        let key_bytes = [0x42u8; 32];
        let encoding_aes_key = base64::engine::general_purpose::STANDARD
            .encode(key_bytes)
            .trim_end_matches('=')
            .to_string();
        assert_eq!(encoding_aes_key.len(), 43);
        WxBizMsgCrypt::new("callback-token", &encoding_aes_key, "wwcorp123").unwrap()
    }

    #[test]
    fn encoding_aes_key_rejects_wrong_length() {
        let err = WxBizMsgCrypt::new("t", "short", "id").unwrap_err();
        assert!(err.contains("43 characters"));
    }

    #[test]
    fn encrypt_decrypt_round_trip() {
        let crypt = sample_crypt();
        let xml = "<xml><Content><![CDATA[hello wecom]]></Content></xml>";
        let encrypted = crypt.encrypt(xml).unwrap();
        let decrypted = crypt.decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, xml);
    }

    #[test]
    fn verify_and_decrypt_checks_signature() {
        let crypt = sample_crypt();
        let xml = "<xml><FromUserName><![CDATA[zhangsan]]></FromUserName></xml>";
        let encrypt = crypt.encrypt(xml).unwrap();
        let ts = "1409304348";
        let nonce = "nonce123";
        let sig = crypt.signature(ts, nonce, &encrypt);
        let out = crypt.verify_and_decrypt(&sig, ts, nonce, &encrypt).unwrap();
        assert_eq!(out, xml);
        let err = crypt
            .verify_and_decrypt("deadbeef", ts, nonce, &encrypt)
            .unwrap_err();
        assert!(err.contains("msg_signature"));
    }

    #[test]
    fn decrypt_rejects_wrong_corpid() {
        let crypt = sample_crypt();
        let xml = "<xml/>";
        let encrypt = crypt.encrypt(xml).unwrap();
        let other = WxBizMsgCrypt::new(
            "callback-token",
            &encoding_key_from_bytes(&[0x42u8; 32]),
            "othercorp",
        )
        .unwrap();
        let err = other.decrypt(&encrypt).unwrap_err();
        assert!(err.contains("receive id"));
    }

    fn encoding_key_from_bytes(bytes: &[u8; 32]) -> String {
        base64::engine::general_purpose::STANDARD
            .encode(bytes)
            .trim_end_matches('=')
            .to_string()
    }

    #[test]
    fn signature_sorts_parts() {
        let sig = sha1_signature("b", "a", "c", "d");
        let mut hasher = Sha1::new();
        hasher.update(b"abcd");
        assert_eq!(sig, hex_encode(&hasher.finalize()));
    }
}
