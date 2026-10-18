//! Concrete incremental SHA-256 provider shared by receipt and artifact paths.

#[cfg(feature = "verification-sha-openssl")]
pub(crate) struct Sha256(openssl::sha::Sha256);

#[cfg(not(feature = "verification-sha-openssl"))]
pub(crate) struct Sha256(sha2::Sha256);

impl Sha256 {
    pub(crate) fn new() -> Self {
        #[cfg(feature = "verification-sha-openssl")]
        {
            Self(openssl::sha::Sha256::new())
        }
        #[cfg(not(feature = "verification-sha-openssl"))]
        {
            use sha2::Digest;
            Self(sha2::Sha256::new())
        }
    }

    pub(crate) fn update(&mut self, bytes: &[u8]) {
        #[cfg(not(feature = "verification-sha-openssl"))]
        use sha2::Digest;
        self.0.update(bytes);
    }

    pub(crate) fn finish(self) -> [u8; 32] {
        #[cfg(feature = "verification-sha-openssl")]
        {
            self.0.finish()
        }
        #[cfg(not(feature = "verification-sha-openssl"))]
        {
            use sha2::Digest;
            self.0.finalize().into()
        }
    }
}

pub(crate) fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(bytes);
    digest.finish()
}

pub(crate) fn hex(bytes: &[u8; 32]) -> String {
    const LOWER: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(64);
    for byte in bytes {
        output.push(LOWER[(byte >> 4) as usize] as char);
        output.push(LOWER[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::{Sha256, hex, sha256};
    use sha2::{Digest, Sha256 as RustCryptoSha256};

    #[test]
    fn standard_sha256_vectors_match() {
        assert_eq!(
            hex(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn lowercase_hex_matches_independent_format_for_every_byte() {
        let bytes: Vec<u8> = (0..=u8::MAX).collect();
        let mut actual = String::new();
        let mut expected = String::new();
        for chunk in bytes.chunks_exact(32) {
            let digest: &[u8; 32] = chunk.try_into().unwrap();
            actual.push_str(&hex(digest));
            for byte in digest {
                expected.push_str(&format!("{byte:02x}"));
            }
        }
        assert_eq!(actual, expected);
    }

    #[test]
    fn provider_matches_rustcrypto_across_sizes_and_chunk_boundaries() {
        let sizes = [0, 1, 55, 56, 63, 64, 65, 127, 128, 1024 * 1024];
        let chunk_sizes = [1, 7, 55, 56, 63, 64, 65, 127, 128, 4093];
        for size in sizes {
            let input: Vec<u8> = (0..size).map(|index| (index % 251) as u8).collect();
            let expected = RustCryptoSha256::digest(&input);
            for chunk_size in chunk_sizes {
                let mut candidate = Sha256::new();
                for chunk in input.chunks(chunk_size) {
                    candidate.update(chunk);
                }
                assert_eq!(
                    candidate.finish().as_slice(),
                    expected.as_slice(),
                    "size={size} chunk_size={chunk_size}"
                );
            }
        }
    }
}
