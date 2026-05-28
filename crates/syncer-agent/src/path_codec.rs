use std::fmt::Write as _;

use syncer_core::RelativePath;

#[must_use]
pub fn encode_relative_path(path: &RelativePath) -> String {
    let mut encoded = String::new();
    for byte in path.as_path().as_str().bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                encoded.push(char::from(byte));
            }
            other => {
                let _ = write!(&mut encoded, "%{other:02X}");
            }
        }
    }
    encoded
}

pub fn decode_relative_path(value: &str) -> Result<RelativePath, PathDecodeError> {
    let mut bytes = Vec::with_capacity(value.len());
    let input = value.as_bytes();
    let mut index = 0;

    while index < input.len() {
        if input[index] == b'%' {
            let high = input
                .get(index + 1)
                .copied()
                .ok_or(PathDecodeError::InvalidPercentEncoding)?;
            let low = input
                .get(index + 2)
                .copied()
                .ok_or(PathDecodeError::InvalidPercentEncoding)?;
            bytes.push(
                hex_value(high)
                    .and_then(|high| hex_value(low).map(|low| (high << 4) | low))
                    .ok_or(PathDecodeError::InvalidPercentEncoding)?,
            );
            index += 3;
        } else {
            bytes.push(input[index]);
            index += 1;
        }
    }

    let decoded = String::from_utf8(bytes).map_err(|_| PathDecodeError::InvalidUtf8)?;
    RelativePath::parse(decoded).map_err(PathDecodeError::Core)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PathDecodeError {
    #[error("invalid percent encoding")]
    InvalidPercentEncoding,
    #[error("path is not valid UTF-8")]
    InvalidUtf8,
    #[error("{0}")]
    Core(#[from] syncer_core::CoreError),
}

#[cfg(test)]
mod tests {
    use syncer_core::RelativePath;

    use super::{decode_relative_path, encode_relative_path};

    #[test]
    fn round_trips_safe_relative_paths() -> Result<(), Box<dyn std::error::Error>> {
        let path = RelativePath::parse("docs/space file #1.txt")?;
        let encoded = encode_relative_path(&path);
        let decoded = decode_relative_path(&encoded)?;

        assert_eq!(encoded, "docs/space%20file%20%231.txt");
        assert_eq!(decoded, path);
        Ok(())
    }

    #[test]
    fn rejects_traversal_after_decoding() {
        let decoded = decode_relative_path("docs/%2E%2E/secret.txt");

        assert!(decoded.is_err());
    }
}
