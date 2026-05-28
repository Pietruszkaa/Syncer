use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

pub fn sign_request(
    method: &str,
    path: &str,
    device_id: &str,
    timestamp_unix: i64,
    body: &[u8],
    shared_secret_hex: &str,
) -> Result<String, SigningError> {
    let body_hash = Sha256::digest(body);
    sign_request_with_body_hash_hex(
        method,
        path,
        device_id,
        timestamp_unix,
        &hex::encode(body_hash),
        shared_secret_hex,
    )
}

pub fn sign_request_with_body_hash_hex(
    method: &str,
    path: &str,
    device_id: &str,
    timestamp_unix: i64,
    body_hash_hex: &str,
    shared_secret_hex: &str,
) -> Result<String, SigningError> {
    let secret = hex::decode(shared_secret_hex)?;
    let mut mac = HmacSha256::new_from_slice(&secret).map_err(|_| SigningError::InvalidSecret)?;
    mac.update(
        canonical_payload_with_body_hash_hex(
            method,
            path,
            device_id,
            timestamp_unix,
            body_hash_hex,
        )
        .as_bytes(),
    );
    Ok(hex::encode(mac.finalize().into_bytes()))
}

#[cfg(test)]
pub fn verify_request_signature(
    signature_hex: &str,
    method: &str,
    path: &str,
    device_id: &str,
    timestamp_unix: i64,
    body: &[u8],
    shared_secret_hex: &str,
) -> Result<(), SigningError> {
    let body_hash = Sha256::digest(body);
    verify_request_signature_with_body_hash_hex(
        signature_hex,
        method,
        path,
        device_id,
        timestamp_unix,
        &hex::encode(body_hash),
        shared_secret_hex,
    )
}

pub fn verify_request_signature_with_body_hash_hex(
    signature_hex: &str,
    method: &str,
    path: &str,
    device_id: &str,
    timestamp_unix: i64,
    body_hash_hex: &str,
    shared_secret_hex: &str,
) -> Result<(), SigningError> {
    let secret = hex::decode(shared_secret_hex)?;
    let signature = hex::decode(signature_hex)?;
    let mut mac = HmacSha256::new_from_slice(&secret).map_err(|_| SigningError::InvalidSecret)?;
    mac.update(
        canonical_payload_with_body_hash_hex(
            method,
            path,
            device_id,
            timestamp_unix,
            body_hash_hex,
        )
        .as_bytes(),
    );
    mac.verify_slice(&signature)
        .map_err(|_| SigningError::InvalidSignature)
}

fn canonical_payload_with_body_hash_hex(
    method: &str,
    path: &str,
    device_id: &str,
    timestamp_unix: i64,
    body_hash_hex: &str,
) -> String {
    format!("{method}\n{path}\n{device_id}\n{timestamp_unix}\n{body_hash_hex}")
}

#[derive(Debug, thiserror::Error)]
pub enum SigningError {
    #[error("hex decode error: {0}")]
    Hex(#[from] hex::FromHexError),
    #[error("invalid shared secret")]
    InvalidSecret,
    #[error("invalid request signature")]
    InvalidSignature,
}

#[cfg(test)]
mod tests {
    use super::{sign_request, verify_request_signature};

    #[test]
    fn verifies_matching_signature() -> Result<(), Box<dyn std::error::Error>> {
        let secret = "1111111111111111111111111111111111111111111111111111111111111111";
        let signature = sign_request("POST", "/presence", "device", 1, b"{}", secret)?;

        verify_request_signature(&signature, "POST", "/presence", "device", 1, b"{}", secret)?;
        Ok(())
    }
}
