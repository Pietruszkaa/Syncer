use std::fs::File;
use std::io::{Read, Write};

use camino::Utf8Path;
use chacha20poly1305::aead::{AeadInPlace, KeyInit};
use chacha20poly1305::{Key, Tag, XChaCha20Poly1305, XNonce};
use rand::random;
use syncer_core::RelativePath;

const CHUNK_SIZE: usize = 64 * 1024;
const TAG_SIZE: usize = 16;
const NONCE_PREFIX_BYTES: usize = 16;
pub const ENCRYPTION_ALGORITHM: &str = "xchacha20poly1305-blake3-v1";

#[derive(Clone, Debug)]
pub struct FileCipherContext<'context> {
    pub direction: &'context str,
    pub path: &'context RelativePath,
    pub content_hash: &'context str,
    pub plaintext_size: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CryptographicFile {
    pub size_bytes: u64,
    pub content_hash: String,
    pub nonce_hex: String,
}

pub fn encrypted_len(plaintext_len: u64) -> u64 {
    plaintext_len.saturating_add(chunk_count(plaintext_len).saturating_mul(TAG_SIZE as u64))
}

pub fn encrypt_file_to_path(
    source: &Utf8Path,
    destination: &Utf8Path,
    shared_secret_hex: &str,
    context: &FileCipherContext<'_>,
) -> Result<CryptographicFile, PeerCryptoError> {
    let key = derive_key(shared_secret_hex)?;
    let nonce_prefix: [u8; NONCE_PREFIX_BYTES] = random();
    let cipher = XChaCha20Poly1305::new(Key::from_slice(&key));
    let mut input = File::open(source)?;
    let mut output = File::create(destination)?;
    let mut hasher = blake3::Hasher::new();
    let mut processed = 0_u64;
    let mut chunk_index = 0_u64;

    loop {
        let mut chunk = vec![0_u8; CHUNK_SIZE];
        let read = input.read(&mut chunk)?;
        chunk.truncate(read);
        if read == 0 && (context.plaintext_size != 0 || chunk_index != 0) {
            break;
        }

        processed = processed.saturating_add(read as u64);
        hasher.update(&chunk);
        let nonce = chunk_nonce(&nonce_prefix, chunk_index);
        let tag = cipher
            .encrypt_in_place_detached(
                XNonce::from_slice(&nonce),
                chunk_aad(context, chunk_index).as_bytes(),
                &mut chunk,
            )
            .map_err(|_| PeerCryptoError::Encrypt)?;
        output.write_all(&chunk)?;
        output.write_all(tag.as_slice())?;
        chunk_index = chunk_index.saturating_add(1);

        if read == 0 {
            break;
        }
    }

    output.sync_all()?;
    if processed != context.plaintext_size {
        return Err(PeerCryptoError::UnexpectedPlaintextSize {
            expected: context.plaintext_size,
            actual: processed,
        });
    }

    Ok(CryptographicFile {
        size_bytes: encrypted_len(processed),
        content_hash: hasher.finalize().to_hex().to_string(),
        nonce_hex: hex::encode(nonce_prefix),
    })
}

pub fn decrypt_file_to_path(
    source: &Utf8Path,
    destination: &Utf8Path,
    shared_secret_hex: &str,
    nonce_hex: &str,
    context: &FileCipherContext<'_>,
) -> Result<CryptographicFile, PeerCryptoError> {
    let key = derive_key(shared_secret_hex)?;
    let nonce_prefix = decode_nonce_prefix(nonce_hex)?;
    let cipher = XChaCha20Poly1305::new(Key::from_slice(&key));
    let mut input = File::open(source)?;
    let mut output = File::create(destination)?;
    let mut hasher = blake3::Hasher::new();
    let mut remaining = context.plaintext_size;
    let chunks = chunk_count(context.plaintext_size);

    for chunk_index in 0..chunks {
        let plaintext_len = if remaining == 0 {
            0
        } else {
            remaining.min(CHUNK_SIZE as u64)
        };
        let encrypted_len = usize::try_from(plaintext_len.saturating_add(TAG_SIZE as u64))
            .map_err(|_| PeerCryptoError::SizeOverflow)?;
        let mut encrypted = vec![0_u8; encrypted_len];
        input.read_exact(&mut encrypted)?;
        let tag_offset = encrypted
            .len()
            .checked_sub(TAG_SIZE)
            .ok_or(PeerCryptoError::InvalidCiphertext)?;
        let tag_bytes = encrypted.split_off(tag_offset);
        let tag = Tag::from_slice(&tag_bytes);
        let nonce = chunk_nonce(&nonce_prefix, chunk_index);
        cipher
            .decrypt_in_place_detached(
                XNonce::from_slice(&nonce),
                chunk_aad(context, chunk_index).as_bytes(),
                &mut encrypted,
                tag,
            )
            .map_err(|_| PeerCryptoError::Decrypt)?;
        output.write_all(&encrypted)?;
        hasher.update(&encrypted);
        remaining = remaining.saturating_sub(plaintext_len);
    }

    let mut trailing = [0_u8; 1];
    if input.read(&mut trailing)? != 0 {
        return Err(PeerCryptoError::InvalidCiphertext);
    }

    output.sync_all()?;
    Ok(CryptographicFile {
        size_bytes: context.plaintext_size,
        content_hash: hasher.finalize().to_hex().to_string(),
        nonce_hex: nonce_hex.to_owned(),
    })
}

fn derive_key(shared_secret_hex: &str) -> Result<[u8; 32], PeerCryptoError> {
    let secret = hex::decode(shared_secret_hex)?;
    if secret.len() != 32 {
        return Err(PeerCryptoError::InvalidSharedSecret);
    }
    Ok(blake3::derive_key(
        "syncer file transfer encryption v1",
        &secret,
    ))
}

fn decode_nonce_prefix(nonce_hex: &str) -> Result<[u8; NONCE_PREFIX_BYTES], PeerCryptoError> {
    let bytes = hex::decode(nonce_hex)?;
    bytes.try_into().map_err(|_| PeerCryptoError::InvalidNonce)
}

fn chunk_nonce(prefix: &[u8; NONCE_PREFIX_BYTES], chunk_index: u64) -> [u8; 24] {
    let mut nonce = [0_u8; 24];
    nonce[..NONCE_PREFIX_BYTES].copy_from_slice(prefix);
    nonce[NONCE_PREFIX_BYTES..].copy_from_slice(&chunk_index.to_be_bytes());
    nonce
}

fn chunk_count(plaintext_len: u64) -> u64 {
    if plaintext_len == 0 {
        1
    } else {
        plaintext_len.div_ceil(CHUNK_SIZE as u64)
    }
}

fn chunk_aad(context: &FileCipherContext<'_>, chunk_index: u64) -> String {
    format!(
        "{}\n{}\n{}\n{}\n{}",
        context.direction,
        context.path.as_path().as_str(),
        context.content_hash,
        context.plaintext_size,
        chunk_index
    )
}

#[derive(Debug, thiserror::Error)]
pub enum PeerCryptoError {
    #[error("hex decode error: {0}")]
    Hex(#[from] hex::FromHexError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid shared secret")]
    InvalidSharedSecret,
    #[error("invalid nonce")]
    InvalidNonce,
    #[error("encryption failed")]
    Encrypt,
    #[error("decryption failed")]
    Decrypt,
    #[error("invalid ciphertext")]
    InvalidCiphertext,
    #[error("file size exceeded supported range")]
    SizeOverflow,
    #[error("unexpected plaintext size: expected {expected}, got {actual}")]
    UnexpectedPlaintextSize { expected: u64, actual: u64 },
}

#[cfg(test)]
mod tests {
    use std::fs;

    use syncer_core::RelativePath;

    use super::{FileCipherContext, decrypt_file_to_path, encrypt_file_to_path, encrypted_len};

    #[test]
    fn encrypts_and_decrypts_file_chunks() -> Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!("syncer-crypto-{}", uuid::Uuid::now_v7()));
        fs::create_dir_all(&root)?;
        let source = camino::Utf8PathBuf::from_path_buf(root.join("plain.bin"))
            .map_err(|_| "non utf8 path")?;
        let encrypted = camino::Utf8PathBuf::from_path_buf(root.join("encrypted.bin"))
            .map_err(|_| "non utf8 path")?;
        let decrypted = camino::Utf8PathBuf::from_path_buf(root.join("decrypted.bin"))
            .map_err(|_| "non utf8 path")?;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&vec![7_u8; 70 * 1024]);
        bytes.extend_from_slice(b"tail");
        fs::write(&source, &bytes)?;
        let path = RelativePath::parse("docs/plain.bin")?;
        let content_hash = blake3::hash(&bytes).to_hex().to_string();
        let secret = "1111111111111111111111111111111111111111111111111111111111111111";
        let context = FileCipherContext {
            direction: "test",
            path: &path,
            content_hash: &content_hash,
            plaintext_size: bytes.len() as u64,
        };

        let encrypted_file = encrypt_file_to_path(&source, &encrypted, secret, &context)?;
        let decrypted_file = decrypt_file_to_path(
            &encrypted,
            &decrypted,
            secret,
            &encrypted_file.nonce_hex,
            &context,
        )?;

        assert_eq!(encrypted_file.size_bytes, encrypted_len(bytes.len() as u64));
        assert_eq!(decrypted_file.content_hash, content_hash);
        assert_eq!(fs::read(decrypted)?, bytes);
        fs::remove_dir_all(root).ok();
        Ok(())
    }
}
