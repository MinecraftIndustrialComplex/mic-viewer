// SPDX-License-Identifier: MPL-2.0
//! Just enough of Minecraft's NBT to read Surveyor's files: big-endian, every tag type, nesting
//! bounded and every length checked against the bytes left, so a corrupt or hostile file is an error
//! and never a giant allocation.

use std::io::{self, Read};

/// A tag's payload. Compounds keep their order.
#[derive(Clone, Debug, PartialEq)]
pub enum Tag {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<u8>),
    String(String),
    List(Vec<Tag>),
    Compound(Vec<(String, Tag)>),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

impl Tag {
    /// A compound's entry.
    pub fn get(&self, key: &str) -> Option<&Tag> {
        self.entries().iter().find(|(k, _)| k == key).map(|(_, t)| t)
    }

    /// A compound's entries; empty for any other tag.
    pub fn entries(&self) -> &[(String, Tag)] {
        match self {
            Tag::Compound(v) => v,
            _ => &[],
        }
    }

    /// A list's items; empty for any other tag.
    pub fn items(&self) -> &[Tag] {
        match self {
            Tag::List(v) => v,
            _ => &[],
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Tag::String(s) => Some(s),
            _ => None,
        }
    }
}

/// Nesting deeper than this is an error (Surveyor's files nest five deep).
pub const MAX_DEPTH: usize = 64;

fn invalid(what: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.into())
}

/// A gzip file's root compound, refusing more than `cap` bytes once inflated.
pub fn read_gzip(r: impl Read, cap: u64) -> io::Result<Tag> {
    let mut buf = Vec::new();
    flate2::read::GzDecoder::new(r).take(cap + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > cap {
        return Err(invalid(format!("inflates past {cap} bytes")));
    }
    read(&buf)
}

/// An uncompressed file's root compound (its name is dropped).
pub fn read(bytes: &[u8]) -> io::Result<Tag> {
    let mut c = Cursor { b: bytes, i: 0 };
    let t = c.u8()?;
    if t != 10 {
        return Err(invalid(format!("root tag {t}, not a compound")));
    }
    c.string()?;
    c.payload(10, 0)
}

struct Cursor<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> io::Result<&'a [u8]> {
        if self.b.len() - self.i < n {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "NBT ends early"));
        }
        let s = &self.b[self.i..self.i + n];
        self.i += n;
        Ok(s)
    }

    fn array<const N: usize>(&mut self) -> io::Result<[u8; N]> {
        let mut a = [0; N];
        a.copy_from_slice(self.take(N)?);
        Ok(a)
    }

    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn i32(&mut self) -> io::Result<i32> {
        Ok(i32::from_be_bytes(self.array()?))
    }

    /// A length prefix, checked against the bytes left at `elem` bytes an element at least.
    fn len(&mut self, elem: usize) -> io::Result<usize> {
        let n = self.i32()?;
        let n = usize::try_from(n).map_err(|_| invalid(format!("negative length {n}")))?;
        if n.saturating_mul(elem) > self.b.len() - self.i {
            return Err(invalid(format!("length {n} runs past the end")));
        }
        Ok(n)
    }

    fn string(&mut self) -> io::Result<String> {
        let n = u16::from_be_bytes(self.array()?) as usize;
        // Java writes modified UTF-8; Surveyor's strings are ids, so plain UTF-8 reads them.
        Ok(String::from_utf8_lossy(self.take(n)?).into_owned())
    }

    fn payload(&mut self, t: u8, depth: usize) -> io::Result<Tag> {
        if depth > MAX_DEPTH {
            return Err(invalid("nested too deep"));
        }
        Ok(match t {
            1 => Tag::Byte(self.u8()? as i8),
            2 => Tag::Short(i16::from_be_bytes(self.array()?)),
            3 => Tag::Int(self.i32()?),
            4 => Tag::Long(i64::from_be_bytes(self.array()?)),
            5 => Tag::Float(f32::from_be_bytes(self.array()?)),
            6 => Tag::Double(f64::from_be_bytes(self.array()?)),
            7 => {
                let n = self.len(1)?;
                Tag::ByteArray(self.take(n)?.to_vec())
            }
            8 => Tag::String(self.string()?),
            9 => {
                let et = self.u8()?;
                let min = match et {
                    0 => 0,
                    1 | 10 => 1,
                    2 | 8 => 2,
                    3 | 5 | 7 | 11 | 12 => 4,
                    4 | 6 => 8,
                    9 => 5,
                    _ => return Err(invalid(format!("list of tag {et}"))),
                };
                let n = self.len(min)?;
                if et == 0 && n > 0 {
                    return Err(invalid("a list of End tags that is not empty"));
                }
                let mut v = Vec::with_capacity(n);
                for _ in 0..n {
                    v.push(self.payload(et, depth + 1)?);
                }
                Tag::List(v)
            }
            10 => {
                let mut v = Vec::new();
                loop {
                    let tt = self.u8()?;
                    if tt == 0 {
                        break;
                    }
                    let k = self.string()?;
                    v.push((k, self.payload(tt, depth + 1)?));
                }
                Tag::Compound(v)
            }
            11 => {
                let n = self.len(4)?;
                Tag::IntArray(self.take(n * 4)?.as_chunks::<4>().0.iter().map(|c| i32::from_be_bytes(*c)).collect())
            }
            12 => {
                let n = self.len(8)?;
                Tag::LongArray(self.take(n * 8)?.as_chunks::<8>().0.iter().map(|c| i64::from_be_bytes(*c)).collect())
            }
            _ => return Err(invalid(format!("tag type {t}"))),
        })
    }
}

/// Writes NBT (tests and fixtures only: the viewer never writes a world).
pub mod write {
    use super::Tag;

    fn id(t: &Tag) -> u8 {
        match t {
            Tag::Byte(_) => 1,
            Tag::Short(_) => 2,
            Tag::Int(_) => 3,
            Tag::Long(_) => 4,
            Tag::Float(_) => 5,
            Tag::Double(_) => 6,
            Tag::ByteArray(_) => 7,
            Tag::String(_) => 8,
            Tag::List(_) => 9,
            Tag::Compound(_) => 10,
            Tag::IntArray(_) => 11,
            Tag::LongArray(_) => 12,
        }
    }

    fn string(out: &mut Vec<u8>, s: &str) {
        out.extend_from_slice(&(s.len() as u16).to_be_bytes());
        out.extend_from_slice(s.as_bytes());
    }

    fn payload(out: &mut Vec<u8>, t: &Tag) {
        match t {
            Tag::Byte(v) => out.push(*v as u8),
            Tag::Short(v) => out.extend_from_slice(&v.to_be_bytes()),
            Tag::Int(v) => out.extend_from_slice(&v.to_be_bytes()),
            Tag::Long(v) => out.extend_from_slice(&v.to_be_bytes()),
            Tag::Float(v) => out.extend_from_slice(&v.to_be_bytes()),
            Tag::Double(v) => out.extend_from_slice(&v.to_be_bytes()),
            Tag::ByteArray(v) => {
                out.extend_from_slice(&(v.len() as i32).to_be_bytes());
                out.extend_from_slice(v);
            }
            Tag::String(s) => string(out, s),
            Tag::List(v) => {
                out.push(v.first().map_or(0, id));
                out.extend_from_slice(&(v.len() as i32).to_be_bytes());
                for t in v {
                    payload(out, t);
                }
            }
            Tag::Compound(v) => {
                for (k, t) in v {
                    out.push(id(t));
                    string(out, k);
                    payload(out, t);
                }
                out.push(0);
            }
            Tag::IntArray(v) => {
                out.extend_from_slice(&(v.len() as i32).to_be_bytes());
                for x in v {
                    out.extend_from_slice(&x.to_be_bytes());
                }
            }
            Tag::LongArray(v) => {
                out.extend_from_slice(&(v.len() as i32).to_be_bytes());
                for x in v {
                    out.extend_from_slice(&x.to_be_bytes());
                }
            }
        }
    }

    /// A root compound with an empty name.
    pub fn root(t: &Tag) -> Vec<u8> {
        let mut out = vec![id(t)];
        string(&mut out, "");
        payload(&mut out, t);
        out
    }

    /// The same, gzip compressed (Surveyor's files).
    pub fn root_gzip(t: &Tag) -> Vec<u8> {
        use std::io::Write;
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        e.write_all(&root(t)).unwrap();
        e.finish().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Tag {
        Tag::Compound(vec![
            ("b".into(), Tag::Byte(-3)),
            ("s".into(), Tag::Short(-300)),
            ("i".into(), Tag::Int(1 << 20)),
            ("l".into(), Tag::Long(-1)),
            ("f".into(), Tag::Float(1.5)),
            ("d".into(), Tag::Double(-2.25)),
            ("ba".into(), Tag::ByteArray(vec![0, 255, 7])),
            ("str".into(), Tag::String("minecraft:grass_block".into())),
            ("list".into(), Tag::List(vec![Tag::String("a".into()), Tag::String("b".into())])),
            ("empty".into(), Tag::List(vec![])),
            ("ia".into(), Tag::IntArray(vec![1, -2, 0x7FB238])),
            ("la".into(), Tag::LongArray(vec![i64::MIN, 5])),
            ("c".into(), Tag::Compound(vec![("x".into(), Tag::Int(4))])),
        ])
    }

    #[test]
    fn every_tag_reads_back_as_written() {
        let t = sample();
        assert_eq!(read(&write::root(&t)).unwrap(), t);
        assert_eq!(read_gzip(&write::root_gzip(&t)[..], 1 << 20).unwrap(), t);
        assert_eq!(t.get("str").and_then(Tag::as_str), Some("minecraft:grass_block"));
        assert_eq!(t.get("list").unwrap().items().len(), 2);
        assert!(t.get("nope").is_none());
    }

    #[test]
    fn corrupt_input_is_an_error_not_a_panic_or_a_huge_allocation() {
        let bytes = write::root(&sample());
        for cut in 0..bytes.len() {
            assert!(read(&bytes[..cut]).is_err(), "cut at {cut}");
        }
        // A byte array claiming 2 GB.
        let mut huge = vec![10, 0, 0, 7, 0, 1, b'a'];
        huge.extend_from_slice(&i32::MAX.to_be_bytes());
        assert!(read(&huge).is_err());
        // A list of a billion longs.
        let mut list = vec![10, 0, 0, 9, 0, 1, b'a', 4];
        list.extend_from_slice(&1_000_000_000i32.to_be_bytes());
        assert!(read(&list).is_err());
        // Nesting past the limit.
        let mut deep = vec![10, 0, 0];
        for _ in 0..MAX_DEPTH + 2 {
            deep.extend_from_slice(&[10, 0, 1, b'a']);
        }
        assert!(read(&deep).is_err());
        // Not a compound at the root.
        assert!(read(&[8, 0, 0, 0, 0]).is_err());
        // A gzip bomb is cut at the cap.
        let big = Tag::Compound(vec![("z".into(), Tag::ByteArray(vec![0; 1 << 16]))]);
        assert!(read_gzip(&write::root_gzip(&big)[..], 1 << 12).is_err());
    }
}
