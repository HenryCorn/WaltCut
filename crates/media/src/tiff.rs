//! TIFF 6.0 fields needed to decode uncompressed and PackBits YCbCr images.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum TiffError {
    #[error("not a TIFF file")]
    NotTiff,
    #[error("malformed TIFF: {0}")]
    Malformed(&'static str),
}

pub type Result<T> = std::result::Result<T, TiffError>;

const MAX_FIELD_VALUES: usize = 1 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Type {
    Byte,
    Ascii,
    Short,
    Long,
    Rational,
    SByte,
    Undefined,
    SShort,
    SLong,
    SRational,
    Float,
    Double,
    Ifd,
}

impl Type {
    fn from_code(code: u16) -> Option<Self> {
        Some(match code {
            1 => Self::Byte,
            2 => Self::Ascii,
            3 => Self::Short,
            4 => Self::Long,
            5 => Self::Rational,
            6 => Self::SByte,
            7 => Self::Undefined,
            8 => Self::SShort,
            9 => Self::SLong,
            10 => Self::SRational,
            11 => Self::Float,
            12 => Self::Double,
            13 => Self::Ifd,
            _ => return None,
        })
    }

    fn size(self) -> usize {
        match self {
            Self::Byte | Self::Ascii | Self::SByte | Self::Undefined => 1,
            Self::Short | Self::SShort => 2,
            Self::Long | Self::SLong | Self::Float | Self::Ifd => 4,
            Self::Rational | Self::SRational | Self::Double => 8,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Entry {
    pub tag: u16,
    pub ty: Type,
    pub count: u32,
    value_at: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Ifd {
    pub entries: Vec<Entry>,
}

impl Ifd {
    pub fn get(&self, tag: u16) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.tag == tag)
    }
}

#[derive(Clone, Copy)]
pub struct Tiff<'a> {
    data: &'a [u8],
    big_endian: bool,
}

impl<'a> Tiff<'a> {
    pub fn new(data: &'a [u8]) -> Result<(Self, u64)> {
        let big_endian = match data.get(..4) {
            Some(b"II*\0") => false,
            Some(b"MM\0*") => true,
            _ => return Err(TiffError::NotTiff),
        };
        let tiff = Self { data, big_endian };
        let first = u64::from(tiff.u32_at(4)?);
        Ok((tiff, first))
    }

    fn bytes_at(&self, offset: u64, len: usize) -> Result<&'a [u8]> {
        let start = usize::try_from(offset).map_err(|_| TiffError::Malformed("offset outside the file"))?;
        let end = start.checked_add(len).ok_or(TiffError::Malformed("offset outside the file"))?;
        self.data.get(start..end).ok_or(TiffError::Malformed("offset outside the file"))
    }

    fn u16_of(&self, bytes: [u8; 2]) -> u16 {
        if self.big_endian { u16::from_be_bytes(bytes) } else { u16::from_le_bytes(bytes) }
    }

    fn u32_of(&self, bytes: [u8; 4]) -> u32 {
        if self.big_endian { u32::from_be_bytes(bytes) } else { u32::from_le_bytes(bytes) }
    }

    fn u16_at(&self, offset: u64) -> Result<u16> {
        let b = self.bytes_at(offset, 2)?;
        let pair = b.get(..2).ok_or(TiffError::Malformed("short integer outside the file"))?;
        Ok(self.u16_of([
            *pair.first().ok_or(TiffError::Malformed("short integer outside the file"))?,
            *pair.get(1).ok_or(TiffError::Malformed("short integer outside the file"))?,
        ]))
    }

    fn u32_at(&self, offset: u64) -> Result<u32> {
        let b = self.bytes_at(offset, 4)?;
        let v: [u8; 4] = b.try_into().map_err(|_| TiffError::Malformed("long integer outside the file"))?;
        Ok(self.u32_of(v))
    }

    pub fn read_ifd(&self, offset: u64) -> Result<Ifd> {
        let count = u64::from(self.u16_at(offset)?);
        let mut entries = Vec::with_capacity(usize::try_from(count).map_err(|_| TiffError::Malformed("too many entries"))?);
        for i in 0..count {
            let entry_offset = offset
                .checked_add(2)
                .and_then(|v| i.checked_mul(12).and_then(|step| v.checked_add(step)))
                .ok_or(TiffError::Malformed("directory offset overflow"))?;
            let bytes = self.bytes_at(entry_offset, 12)?;
            let tag = self.u16_of(
                bytes.get(0..2).ok_or(TiffError::Malformed("short directory entry"))?.try_into().map_err(|_| TiffError::Malformed("short directory entry"))?,
            );
            let code = self.u16_of(
                bytes.get(2..4).ok_or(TiffError::Malformed("short directory entry"))?.try_into().map_err(|_| TiffError::Malformed("short directory entry"))?,
            );
            let Some(ty) = Type::from_code(code) else { continue };
            let count_bytes = bytes.get(4..8).ok_or(TiffError::Malformed("short directory entry"))?;
            let value_count = self.u32_of(count_bytes.try_into().map_err(|_| TiffError::Malformed("short directory entry"))?);
            let value_len = usize::try_from(value_count).ok().and_then(|n| n.checked_mul(ty.size())).ok_or(TiffError::Malformed("field too large"))?;
            if value_len > MAX_FIELD_VALUES {
                return Err(TiffError::Malformed("field exceeds the supported size"));
            }
            let inline = bytes.get(8..12).ok_or(TiffError::Malformed("short directory entry"))?;
            let inline_value: [u8; 4] = inline.try_into().map_err(|_| TiffError::Malformed("short directory entry"))?;
            let value_at = if value_len <= 4 {
                entry_offset.checked_add(8).ok_or(TiffError::Malformed("directory offset overflow"))?
            } else {
                u64::from(self.u32_of(inline_value))
            };
            entries.push(Entry { tag, ty, count: value_count, value_at });
        }
        let trailer = offset
            .checked_add(2)
            .and_then(|v| count.checked_mul(12).and_then(|step| v.checked_add(step)))
            .ok_or(TiffError::Malformed("directory offset overflow"))?;
        let _ = self.u32_at(trailer)?;
        Ok(Ifd { entries })
    }

    fn value_bytes(&self, entry: &Entry) -> Result<&'a [u8]> {
        let count = usize::try_from(entry.count).map_err(|_| TiffError::Malformed("field too large"))?;
        if count > MAX_FIELD_VALUES {
            return Err(TiffError::Malformed("field exceeds the supported size"));
        }
        let len = count.checked_mul(entry.ty.size()).ok_or(TiffError::Malformed("field too large"))?;
        self.bytes_at(entry.value_at, len)
    }

    fn number(&self, entry: &Entry, bytes: &[u8], index: usize) -> Result<f64> {
        let size = entry.ty.size();
        let start = index.checked_mul(size).ok_or(TiffError::Malformed("field index overflow"))?;
        let end = start.checked_add(size).ok_or(TiffError::Malformed("field index overflow"))?;
        let value = bytes.get(start..end).ok_or(TiffError::Malformed("entry too short"))?;
        let u16_at = |at: usize| -> Result<u16> {
            let two: [u8; 2] =
                value.get(at..at + 2).ok_or(TiffError::Malformed("entry too short"))?.try_into().map_err(|_| TiffError::Malformed("entry too short"))?;
            Ok(self.u16_of(two))
        };
        let u32_at = |at: usize| -> Result<u32> {
            let four: [u8; 4] =
                value.get(at..at + 4).ok_or(TiffError::Malformed("entry too short"))?.try_into().map_err(|_| TiffError::Malformed("entry too short"))?;
            Ok(self.u32_of(four))
        };
        Ok(match entry.ty {
            Type::Byte | Type::Undefined => f64::from(*value.first().ok_or(TiffError::Malformed("entry too short"))?),
            Type::SByte => f64::from(*value.first().ok_or(TiffError::Malformed("entry too short"))? as i8),
            Type::Short => f64::from(u16_at(0)?),
            Type::SShort => f64::from(u16_at(0)? as i16),
            Type::Long | Type::Ifd => f64::from(u32_at(0)?),
            Type::SLong => f64::from(u32_at(0)? as i32),
            Type::Rational => {
                let denominator = u32_at(4)?;
                if denominator == 0 {
                    return Err(TiffError::Malformed("rational with a zero denominator"));
                }
                f64::from(u32_at(0)?) / f64::from(denominator)
            }
            Type::SRational => {
                let denominator = u32_at(4)? as i32;
                if denominator == 0 {
                    return Err(TiffError::Malformed("rational with a zero denominator"));
                }
                f64::from(u32_at(0)? as i32) / f64::from(denominator)
            }
            Type::Float => f64::from(f32::from_bits(u32_at(0)?)),
            Type::Double => {
                let raw: [u8; 8] = value.try_into().map_err(|_| TiffError::Malformed("entry too short"))?;
                if self.big_endian { f64::from_be_bytes(raw) } else { f64::from_le_bytes(raw) }
            }
            Type::Ascii => return Err(TiffError::Malformed("text where a number belongs")),
        })
    }

    pub fn floats(&self, entry: &Entry) -> Result<Vec<f64>> {
        let bytes = self.value_bytes(entry)?;
        let count = usize::try_from(entry.count).map_err(|_| TiffError::Malformed("field too large"))?;
        let mut out = Vec::with_capacity(count);
        for index in 0..count {
            let value = self.number(entry, bytes, index)?;
            if !value.is_finite() {
                return Err(TiffError::Malformed("non-finite number"));
            }
            out.push(value);
        }
        Ok(out)
    }

    pub fn uints(&self, entry: &Entry) -> Result<Vec<u64>> {
        if !matches!(entry.ty, Type::Byte | Type::Undefined | Type::Short | Type::Long | Type::Ifd) {
            return Err(TiffError::Malformed("expected an unsigned integer field"));
        }
        let bytes = self.value_bytes(entry)?;
        let size = entry.ty.size();
        let mut out = Vec::with_capacity(usize::try_from(entry.count).map_err(|_| TiffError::Malformed("field too large"))?);
        for value in bytes.chunks_exact(size) {
            let number = match size {
                1 => u64::from(*value.first().ok_or(TiffError::Malformed("entry too short"))?),
                2 => {
                    let pair: [u8; 2] = value.try_into().map_err(|_| TiffError::Malformed("entry too short"))?;
                    u64::from(self.u16_of(pair))
                }
                _ => {
                    let four: [u8; 4] = value.try_into().map_err(|_| TiffError::Malformed("entry too short"))?;
                    u64::from(self.u32_of(four))
                }
            };
            out.push(number);
        }
        Ok(out)
    }

    pub fn uint(&self, entry: &Entry) -> Result<u64> {
        self.uints(entry)?.first().copied().ok_or(TiffError::Malformed("empty field"))
    }
}

pub mod tags {
    pub const IMAGE_WIDTH: u16 = 256;
    pub const IMAGE_LENGTH: u16 = 257;
    pub const BITS_PER_SAMPLE: u16 = 258;
    pub const COMPRESSION: u16 = 259;
    pub const PHOTOMETRIC: u16 = 262;
    pub const STRIP_OFFSETS: u16 = 273;
    pub const SAMPLES_PER_PIXEL: u16 = 277;
    pub const STRIP_BYTE_COUNTS: u16 = 279;
    pub const PLANAR_CONFIGURATION: u16 = 284;
    pub const TILE_OFFSETS: u16 = 324;
}
