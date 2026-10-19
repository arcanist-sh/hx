//! SHA-256 hashing shared by every hx crate.
//!
//! Cache keys, package IDs, lockfile fingerprints and download checksums are
//! all lowercase-hex SHA-256. Crates hash through this module (and its
//! re-exported [`Sha256`]/[`Digest`]) so there is one sha2 version and one
//! hex encoding in the build, and file hashing streams instead of each crate
//! carrying its own read loop.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

pub use sha2::{Digest, Sha256};

/// Lowercase hex of a digest (or any bytes).
pub fn to_hex(bytes: impl AsRef<[u8]>) -> String {
    hex::encode(bytes)
}

/// Lowercase hex SHA-256 of `data`.
pub fn sha256_hex(data: impl AsRef<[u8]>) -> String {
    to_hex(Sha256::digest(data))
}

/// Lowercase hex SHA-256 of everything `reader` yields, read in 64 KiB chunks.
pub fn sha256_reader(mut reader: impl Read) -> io::Result<String> {
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        hasher.update(&buf[..n]);
    }
    Ok(to_hex(hasher.finalize()))
}

/// Lowercase hex SHA-256 of a file's contents, streamed.
pub fn sha256_file(path: &Path) -> io::Result<String> {
    sha256_reader(File::open(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    const EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    fn large_input() -> Vec<u8> {
        (0..200_000u32).map(|i| (i % 251) as u8).collect()
    }
    const LARGE: &str = "e24bc62381f1224fbbb74688663f8f9743b9680b193edd666835e97b06e730eb";

    #[test]
    fn known_vectors() {
        assert_eq!(sha256_hex(b"abc"), ABC);
        assert_eq!(sha256_hex(b""), EMPTY);
        assert_eq!(sha256_hex(large_input()), LARGE);
    }

    #[test]
    fn incremental_matches_one_shot() {
        let mut hasher = Sha256::new();
        hasher.update(b"a");
        hasher.update(b"bc");
        assert_eq!(to_hex(hasher.finalize()), ABC);
    }

    #[test]
    fn reader_and_file_stream_across_chunks() {
        assert_eq!(sha256_reader(&large_input()[..]).unwrap(), LARGE);
        assert_eq!(sha256_reader(io::empty()).unwrap(), EMPTY);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blob");
        std::fs::write(&path, large_input()).unwrap();
        assert_eq!(sha256_file(&path).unwrap(), LARGE);
    }

    #[test]
    fn missing_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(sha256_file(&dir.path().join("nope")).is_err());
    }
}
