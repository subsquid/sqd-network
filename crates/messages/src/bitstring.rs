use crate::BitString;

impl BitString {
    pub fn new(slice: &[bool]) -> Self {
        use flate2::write::DeflateEncoder;
        use std::io::Write;

        let byte_slice: &[u8] = bytemuck::cast_slice(slice);
        let mut encoder = DeflateEncoder::new(Vec::new(), flate2::Compression::best());
        encoder.write_all(byte_slice).expect("Couldn't compress data");
        let compressed = encoder.finish().expect("Couldn't finish compressing data");

        Self {
            data: compressed,
            size: slice.len() as u64,
        }
    }

    /// Each byte of the result is either 0 or 1.
    pub fn to_bytes(&self) -> Vec<u8> {
        use flate2::bufread::DeflateDecoder;
        use std::io::Read;

        let mut decoder = DeflateDecoder::new(&self.data[..]);
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed).expect("Couldn't decompress data");
        decompressed
    }

    pub fn to_vec(&self) -> Result<Vec<bool>, &'static str> {
        self.to_bytes()
            .into_iter()
            .map(|byte| match byte {
                0 => Ok(false),
                1 => Ok(true),
                _ => Err("invalid boolean value in parsed BitString"),
            })
            .collect()
    }
}
