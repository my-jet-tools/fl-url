use std::io::Write;

use crate::FlUrlError;

/// How hard [`compress_with_level`] works at a request body: the time it takes to
/// compress traded for the size of what goes on the wire. The same on both backends.
///
/// [`compress_with_level`]: crate::FlUrl::compress_with_level
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompressionLevel {
    /// The least time, the least gain — gzip level 1. For a big body sent often, when
    /// the CPU is worth more than the bytes.
    Fast,
    /// The balance gzip itself defaults to — level 6. What [`compress`] uses.
    ///
    /// [`compress`]: crate::FlUrl::compress
    #[default]
    Default,
    /// The smallest body, the most time — gzip level 9. For a slow or metered link.
    Best,
}

impl CompressionLevel {
    fn to_gzip(self) -> flate2::Compression {
        match self {
            Self::Fast => flate2::Compression::fast(),
            Self::Default => flate2::Compression::default(),
            Self::Best => flate2::Compression::best(),
        }
    }
}

/// The body gzipped at `level`. Both backends compress a request body with this.
pub(crate) fn gzip(body: &[u8], level: CompressionLevel) -> Result<Vec<u8>, FlUrlError> {
    // The encoder writes into a Vec, which does not fail; the io::Result is only
    // the shape of its api.
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), level.to_gzip());
    encoder.write_all(body)?;
    Ok(encoder.finish()?)
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::{gzip, CompressionLevel};

    const LEVELS: [CompressionLevel; 3] = [
        CompressionLevel::Fast,
        CompressionLevel::Default,
        CompressionLevel::Best,
    ];

    fn compressible_body() -> Vec<u8> {
        (0..2000)
            .map(|i| format!("{{\"id\":{},\"name\":\"user-{}\",\"active\":true}},", i, i % 37))
            .collect::<String>()
            .into_bytes()
    }

    fn gunzip(data: &[u8]) -> Vec<u8> {
        let mut result = Vec::new();
        flate2::read::GzDecoder::new(data)
            .read_to_end(&mut result)
            .unwrap();
        result
    }

    #[test]
    fn every_level_gives_a_gzip_of_the_same_body() {
        let body = compressible_body();

        for level in LEVELS {
            let compressed = gzip(&body, level).unwrap();
            assert!(compressed.len() < body.len(), "{:?}", level);
            assert_eq!(gunzip(&compressed), body, "{:?}", level);
        }
    }

    #[test]
    fn best_gives_a_smaller_body_than_fast() {
        let body = compressible_body();

        let fast = gzip(&body, CompressionLevel::Fast).unwrap();
        let best = gzip(&body, CompressionLevel::Best).unwrap();

        assert!(best.len() < fast.len(), "best {} vs fast {}", best.len(), fast.len());
    }

    #[test]
    fn the_default_level_is_default() {
        assert_eq!(CompressionLevel::default(), CompressionLevel::Default);
    }
}
