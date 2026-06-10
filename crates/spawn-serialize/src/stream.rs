//! The direction-agnostic [`Stream`] trait and the [`Serialize`] contract.
//!
//! A type implements **one** [`Serialize::serialize`] function that drives both
//! encoding and decoding: on a [`BitWriter`](crate::bits::BitWriter) the `&mut`
//! arguments are read from; on a [`BitReader`](crate::bits::BitReader) they are
//! written into. Keeping read and write in a single function makes the wire layout
//! identical by construction.

use crate::bits::{BitReader, BitWriter};
use crate::error::SerializeResult;

/// A bit stream that can be either written or read. Implemented by `BitWriter` and
/// `BitReader` so a value's single `serialize` function works in both directions.
pub trait Stream {
    /// `true` on a writer, `false` on a reader. Used by helpers whose encode and
    /// decode forms are not structurally identical (e.g. zig-zag, quantization).
    fn is_writing(&self) -> bool;

    /// On a writer, write the low `width` bits of `*value`; on a reader, read `width`
    /// bits into `*value`. Errors propagate the underlying bit-I/O failure.
    fn serialize_bits(&mut self, value: &mut u64, width: u32) -> SerializeResult<()>;

    /// On a writer, write `*value`; on a reader, read one bit into `*value`.
    fn serialize_bool(&mut self, value: &mut bool) -> SerializeResult<()>;
}

impl Stream for BitWriter<'_> {
    fn is_writing(&self) -> bool {
        true
    }

    fn serialize_bits(&mut self, value: &mut u64, width: u32) -> SerializeResult<()> {
        self.write_bits(*value, width)
    }

    fn serialize_bool(&mut self, value: &mut bool) -> SerializeResult<()> {
        self.write_bool(*value)
    }
}

impl Stream for BitReader<'_> {
    fn is_writing(&self) -> bool {
        false
    }

    fn serialize_bits(&mut self, value: &mut u64, width: u32) -> SerializeResult<()> {
        *value = self.read_bits(width)?;
        Ok(())
    }

    fn serialize_bool(&mut self, value: &mut bool) -> SerializeResult<()> {
        *value = self.read_bool()?;
        Ok(())
    }
}

/// A value with one read/write-symmetric serialization.
pub trait Serialize {
    /// Encode into / decode from `s`, depending on the stream's direction.
    fn serialize<S: Stream>(&mut self, s: &mut S) -> SerializeResult<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack::serialize_int;

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct Sample {
        flag: bool,
        small: u64,
        signed: i64,
    }

    impl Serialize for Sample {
        fn serialize<S: Stream>(&mut self, s: &mut S) -> SerializeResult<()> {
            s.serialize_bool(&mut self.flag)?;
            s.serialize_bits(&mut self.small, 10)?;
            serialize_int(s, &mut self.signed, 16)
        }
    }

    #[test]
    fn serialize_type_round_trips_and_bit_lengths_match() {
        let mut original = Sample {
            flag: true,
            small: 999,
            signed: -1234,
        };
        let mut buf = [0u8; 16];
        let mut w = BitWriter::new(&mut buf);
        original.serialize(&mut w).unwrap();
        let written = w.bits_written();
        let n = w.finish();

        let mut decoded = Sample {
            flag: false,
            small: 0,
            signed: 0,
        };
        let mut r = BitReader::new(&buf[..n]);
        decoded.serialize(&mut r).unwrap();
        assert_eq!(decoded, original);
        assert_eq!(r.bits_read(), written);
    }
}
