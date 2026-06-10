//! `BitWriter` / `BitReader`: bit-precise I/O over a caller-owned byte buffer.
//!
//! Bit order is fixed and host-endianness-independent: bits are packed MSB-first
//! within each byte, bytes ascending. The first bit written lands in bit 7 of byte 0.
//! A value written with [`BitWriter::write_bits`] at width `w` reads back identically
//! with [`BitReader::read_bits`] at the same width.

use crate::error::{SerializeError, SerializeResult};

/// Largest single-call bit width (the carrier is a `u64`).
const MAX_WIDTH: u32 = 64;

fn check_width(width: u32) -> SerializeResult<()> {
    if width == 0 || width > MAX_WIDTH {
        Err(SerializeError::InvalidWidth { width })
    } else {
        Ok(())
    }
}

/// Writes bits MSB-first within each byte into a caller-owned buffer.
///
/// The buffer may contain prior bytes; every written bit is set explicitly (the slot
/// is cleared then assigned), and [`finish`](BitWriter::finish) zero-fills the unused
/// tail of the final partial byte so the produced prefix is fully defined.
pub struct BitWriter<'a> {
    buf: &'a mut [u8],
    bit_pos: usize,
}

impl<'a> BitWriter<'a> {
    /// Wrap `buf` as a fresh writer positioned at bit 0.
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, bit_pos: 0 }
    }

    fn put_bit(&mut self, bit: u8) -> SerializeResult<()> {
        let byte = self.bit_pos / 8;
        if byte >= self.buf.len() {
            return Err(SerializeError::EndOfStream);
        }
        let shift = 7 - (self.bit_pos % 8);
        let mask = 1u8 << shift;
        self.buf[byte] = (self.buf[byte] & !mask) | ((bit & 1) << shift);
        self.bit_pos += 1;
        Ok(())
    }

    /// Write the low `width` bits of `value` (`width ∈ 1..=64`), MSB-first.
    /// `InvalidWidth` if `width` is `0` or `> 64`; `OutOfRange` if `value` has any bit
    /// set at or above `width`; `EndOfStream` if the buffer is exhausted.
    pub fn write_bits(&mut self, value: u64, width: u32) -> SerializeResult<()> {
        check_width(width)?;
        if width < MAX_WIDTH && value >> width != 0 {
            return Err(SerializeError::OutOfRange {
                context: "write_bits: value exceeds width",
            });
        }
        for i in (0..width).rev() {
            self.put_bit(((value >> i) & 1) as u8)?;
        }
        Ok(())
    }

    /// Write a single bit.
    pub fn write_bool(&mut self, value: bool) -> SerializeResult<()> {
        self.put_bit(value as u8)
    }

    /// Byte-align (zero-fill the current partial byte) then copy `bytes` verbatim.
    pub fn write_aligned(&mut self, bytes: &[u8]) -> SerializeResult<()> {
        self.align_to_byte()?;
        let byte = self.bit_pos / 8;
        let end = byte
            .checked_add(bytes.len())
            .ok_or(SerializeError::EndOfStream)?;
        if end > self.buf.len() {
            return Err(SerializeError::EndOfStream);
        }
        self.buf[byte..end].copy_from_slice(bytes);
        self.bit_pos = end * 8;
        Ok(())
    }

    fn align_to_byte(&mut self) -> SerializeResult<()> {
        while !self.bit_pos.is_multiple_of(8) {
            self.put_bit(0)?;
        }
        Ok(())
    }

    /// Number of bits written so far.
    pub fn bits_written(&self) -> usize {
        self.bit_pos
    }

    /// Zero-fill the unused tail of the final partial byte and return the number of
    /// whole bytes written (`ceil(bits_written / 8)`).
    pub fn finish(mut self) -> usize {
        let _ = self.align_to_byte();
        self.bit_pos / 8
    }
}

/// Reads bits written by [`BitWriter`], in the same MSB-first order.
pub struct BitReader<'a> {
    buf: &'a [u8],
    bit_pos: usize,
}

impl<'a> BitReader<'a> {
    /// Wrap `buf` as a fresh reader positioned at bit 0.
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, bit_pos: 0 }
    }

    fn get_bit(&mut self) -> SerializeResult<u8> {
        let byte = self.bit_pos / 8;
        if byte >= self.buf.len() {
            return Err(SerializeError::EndOfStream);
        }
        let shift = 7 - (self.bit_pos % 8);
        let bit = (self.buf[byte] >> shift) & 1;
        self.bit_pos += 1;
        Ok(bit)
    }

    /// Read `width` bits (`width ∈ 1..=64`) into a `u64`, MSB-first.
    /// `InvalidWidth` for a bad width; `EndOfStream` past the end of input.
    pub fn read_bits(&mut self, width: u32) -> SerializeResult<u64> {
        check_width(width)?;
        let mut value = 0u64;
        for _ in 0..width {
            value = (value << 1) | u64::from(self.get_bit()?);
        }
        Ok(value)
    }

    /// Read a single bit as a bool.
    pub fn read_bool(&mut self) -> SerializeResult<bool> {
        Ok(self.get_bit()? != 0)
    }

    /// Byte-align (skip the current partial byte) then copy `out.len()` bytes.
    pub fn read_aligned(&mut self, out: &mut [u8]) -> SerializeResult<()> {
        self.align_to_byte();
        let byte = self.bit_pos / 8;
        let end = byte
            .checked_add(out.len())
            .ok_or(SerializeError::EndOfStream)?;
        if end > self.buf.len() {
            return Err(SerializeError::EndOfStream);
        }
        out.copy_from_slice(&self.buf[byte..end]);
        self.bit_pos = end * 8;
        Ok(())
    }

    fn align_to_byte(&mut self) {
        if !self.bit_pos.is_multiple_of(8) {
            self.bit_pos = (self.bit_pos / 8 + 1) * 8;
        }
    }

    /// Number of bits read so far.
    pub fn bits_read(&self) -> usize {
        self.bit_pos
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_width_at_boundaries() {
        for width in 1..=64u32 {
            let max = if width == 64 {
                u64::MAX
            } else {
                (1u64 << width) - 1
            };
            for value in [0u64, 1, max] {
                let mut buf = [0u8; 16];
                let mut w = BitWriter::new(&mut buf);
                w.write_bits(value, width).unwrap();
                let n = w.finish();
                let mut r = BitReader::new(&buf[..n]);
                assert_eq!(r.read_bits(width).unwrap(), value, "width {width}");
            }
        }
    }

    #[test]
    fn mixed_widths_and_bools_round_trip() {
        let mut buf = [0u8; 8];
        let mut w = BitWriter::new(&mut buf);
        w.write_bits(0b101, 3).unwrap();
        w.write_bool(true).unwrap();
        w.write_bits(0xABCD, 16).unwrap();
        w.write_bool(false).unwrap();
        w.write_bits(0x7, 4).unwrap();
        let written = w.bits_written();
        let n = w.finish();
        let mut r = BitReader::new(&buf[..n]);
        assert_eq!(r.read_bits(3).unwrap(), 0b101);
        assert!(r.read_bool().unwrap());
        assert_eq!(r.read_bits(16).unwrap(), 0xABCD);
        assert!(!r.read_bool().unwrap());
        assert_eq!(r.read_bits(4).unwrap(), 0x7);
        assert_eq!(r.bits_read(), written);
    }

    #[test]
    fn aligned_blocks_after_partial_byte() {
        let mut buf = [0u8; 16];
        let mut w = BitWriter::new(&mut buf);
        w.write_bits(0b11, 2).unwrap();
        w.write_aligned(&[1, 2, 3, 4, 5]).unwrap();
        w.write_bits(0b1010, 4).unwrap();
        let n = w.finish();
        let mut r = BitReader::new(&buf[..n]);
        assert_eq!(r.read_bits(2).unwrap(), 0b11);
        let mut out = [0u8; 5];
        r.read_aligned(&mut out).unwrap();
        assert_eq!(out, [1, 2, 3, 4, 5]);
        assert_eq!(r.read_bits(4).unwrap(), 0b1010);
    }

    #[test]
    fn write_past_buffer_is_end_of_stream() {
        let mut buf = [0u8; 1];
        let mut w = BitWriter::new(&mut buf);
        w.write_bits(0xFF, 8).unwrap();
        assert_eq!(w.write_bool(true), Err(SerializeError::EndOfStream));
    }

    #[test]
    fn read_past_end_is_end_of_stream() {
        let buf = [0xFFu8; 1];
        let mut r = BitReader::new(&buf);
        r.read_bits(8).unwrap();
        assert_eq!(r.read_bool(), Err(SerializeError::EndOfStream));
    }

    #[test]
    fn invalid_width_rejected() {
        let mut buf = [0u8; 16];
        let mut w = BitWriter::new(&mut buf);
        assert_eq!(
            w.write_bits(0, 0),
            Err(SerializeError::InvalidWidth { width: 0 })
        );
        assert_eq!(
            w.write_bits(0, 65),
            Err(SerializeError::InvalidWidth { width: 65 })
        );
        let mut r = BitReader::new(&buf);
        assert_eq!(
            r.read_bits(0),
            Err(SerializeError::InvalidWidth { width: 0 })
        );
    }

    #[test]
    fn over_wide_value_rejected() {
        let mut buf = [0u8; 16];
        let mut w = BitWriter::new(&mut buf);
        assert!(matches!(
            w.write_bits(0b100, 2),
            Err(SerializeError::OutOfRange { .. })
        ));
    }

    #[test]
    fn write_into_dirty_buffer_is_clean() {
        // A buffer full of 1s must not leak into a zero write.
        let mut buf = [0xFFu8; 4];
        let mut w = BitWriter::new(&mut buf);
        w.write_bits(0, 5).unwrap();
        w.write_bits(0b1, 1).unwrap();
        let n = w.finish();
        let mut r = BitReader::new(&buf[..n]);
        assert_eq!(r.read_bits(5).unwrap(), 0);
        assert_eq!(r.read_bits(1).unwrap(), 1);
    }
}
