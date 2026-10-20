//! A serde `Serializer` that feeds a `Hasher` directly: the structure JSON
//! would carry (field names, variant names, sequence lengths, a tag per
//! value kind) without building the JSON text. `canonical_hash` used to
//! `serde_json::to_vec` the view and hash the bytes, which was ~60% of the
//! endgame solver's per-replay time.

use serde::ser::{self, Serialize};
use std::fmt;
use std::hash::Hasher;

#[derive(Debug)]
pub(crate) struct Never;
impl fmt::Display for Never {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("hash serializer error") }
}
impl std::error::Error for Never {}
impl ser::Error for Never {
    fn custom<T: fmt::Display>(_: T) -> Self { Never }
}

pub(crate) struct HashSer<'a, H: Hasher>(pub &'a mut H);

impl<H: Hasher> HashSer<'_, H> {
    fn tag(&mut self, t: u8) { self.0.write_u8(t); }
    fn str(&mut self, s: &str) { self.0.write_usize(s.len()); self.0.write(s.as_bytes()); }
    /// A field or variant name: always a `&'static str` from a derive or
    /// hand-written impl, so its address identifies it within the process
    /// (the hash is never persisted). One word instead of the bytes.
    fn name(&mut self, s: &'static str) { self.0.write_usize(s.as_ptr() as usize ^ s.len().rotate_left(48)); }
}

macro_rules! prim {
    ($($f:ident $t:ty => $tag:expr, $w:ident;)*) => {$(
        fn $f(self, v: $t) -> Result<(), Never> { self.tag($tag); self.0.$w(v as _); Ok(()) }
    )*};
}

impl<'a, 'b, H: Hasher> ser::Serializer for &'b mut HashSer<'a, H> {
    type Ok = ();
    type Error = Never;
    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = Self;

    prim! {
        serialize_i8 i8 => 1, write_i64; serialize_i16 i16 => 1, write_i64;
        serialize_i32 i32 => 1, write_i64; serialize_i64 i64 => 1, write_i64;
        serialize_u8 u8 => 2, write_u64; serialize_u16 u16 => 2, write_u64;
        serialize_u32 u32 => 2, write_u64; serialize_u64 u64 => 2, write_u64;
        serialize_char char => 3, write_u32;
    }
    fn serialize_bool(self, v: bool) -> Result<(), Never> { self.tag(4); self.0.write_u8(v as u8); Ok(()) }
    fn serialize_f32(self, v: f32) -> Result<(), Never> { self.tag(5); self.0.write_u64((v as f64).to_bits()); Ok(()) }
    fn serialize_f64(self, v: f64) -> Result<(), Never> { self.tag(5); self.0.write_u64(v.to_bits()); Ok(()) }
    fn serialize_str(self, v: &str) -> Result<(), Never> { self.tag(6); self.str(v); Ok(()) }
    fn serialize_bytes(self, v: &[u8]) -> Result<(), Never> { self.tag(7); self.0.write_usize(v.len()); self.0.write(v); Ok(()) }
    fn serialize_none(self) -> Result<(), Never> { self.tag(8); Ok(()) }
    fn serialize_some<T: ?Sized + Serialize>(self, v: &T) -> Result<(), Never> { self.tag(9); v.serialize(self) }
    fn serialize_unit(self) -> Result<(), Never> { self.tag(10); Ok(()) }
    fn serialize_unit_struct(self, _: &'static str) -> Result<(), Never> { self.tag(10); Ok(()) }
    fn serialize_unit_variant(self, _: &'static str, i: u32, v: &'static str) -> Result<(), Never> {
        self.tag(11); self.0.write_u32(i); self.name(v); Ok(())
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(self, _: &'static str, v: &T) -> Result<(), Never> { v.serialize(self) }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(self, _: &'static str, i: u32, n: &'static str, v: &T) -> Result<(), Never> {
        self.tag(12); self.0.write_u32(i); self.name(n); v.serialize(self)
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<Self, Never> {
        self.tag(13); self.0.write_usize(len.unwrap_or(usize::MAX)); Ok(self)
    }
    fn serialize_tuple(self, len: usize) -> Result<Self, Never> { self.tag(14); self.0.write_usize(len); Ok(self) }
    fn serialize_tuple_struct(self, _: &'static str, len: usize) -> Result<Self, Never> { self.tag(14); self.0.write_usize(len); Ok(self) }
    fn serialize_tuple_variant(self, _: &'static str, i: u32, n: &'static str, len: usize) -> Result<Self, Never> {
        self.tag(15); self.0.write_u32(i); self.name(n); self.0.write_usize(len); Ok(self)
    }
    fn serialize_map(self, len: Option<usize>) -> Result<Self, Never> {
        self.tag(16); self.0.write_usize(len.unwrap_or(usize::MAX)); Ok(self)
    }
    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Self, Never> { self.tag(17); Ok(self) }
    fn serialize_struct_variant(self, _: &'static str, i: u32, n: &'static str, _: usize) -> Result<Self, Never> {
        self.tag(18); self.0.write_u32(i); self.name(n); Ok(self)
    }
}

// Unknown-length sequences and maps, and structs (whose skipped fields
// change the field count), close with an end marker so concatenations
// can't collide.
macro_rules! compound {
    ($tr:ident, $m:ident $(, $k:ident)?) => {
        impl<'a, 'b, H: Hasher> ser::$tr for &'b mut HashSer<'a, H> {
            type Ok = ();
            type Error = Never;
            fn $m<T: ?Sized + Serialize>(&mut self, $($k: &'static str,)? v: &T) -> Result<(), Never> {
                $( self.name($k); )?
                v.serialize(&mut **self)
            }
            fn end(self) -> Result<(), Never> { self.tag(0xFF); Ok(()) }
        }
    };
}
compound!(SerializeSeq, serialize_element);
compound!(SerializeTuple, serialize_element);
compound!(SerializeTupleStruct, serialize_field);
compound!(SerializeTupleVariant, serialize_field);
compound!(SerializeStruct, serialize_field, key);
compound!(SerializeStructVariant, serialize_field, key);

impl<'a, 'b, H: Hasher> ser::SerializeMap for &'b mut HashSer<'a, H> {
    type Ok = ();
    type Error = Never;
    fn serialize_key<T: ?Sized + Serialize>(&mut self, k: &T) -> Result<(), Never> { k.serialize(&mut **self) }
    fn serialize_value<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), Never> { v.serialize(&mut **self) }
    fn end(self) -> Result<(), Never> { self.tag(0xFF); Ok(()) }
}

/// Folded-multiply hasher (the ahash / foldhash mixing step) for
/// `canonical_hash`. The serializer above issues thousands of tiny writes
/// per battle; SipHash's per-write cost made it the solver's hottest
/// function. Every word goes through a full 64×64→128 multiply-fold, so
/// there is no structured (Fx-style) collision.
pub(crate) struct FoldHasher(u64);

const K1: u64 = 0x243f_6a88_85a3_08d3;
const K2: u64 = 0x1319_8a2e_0370_7344;

#[inline]
fn fold_mul(a: u64, b: u64) -> u64 {
    let r = (a as u128).wrapping_mul(b as u128);
    (r as u64) ^ ((r >> 64) as u64)
}

impl FoldHasher {
    pub(crate) fn new() -> Self { FoldHasher(K2) }
}

impl Hasher for FoldHasher {
    #[inline]
    fn write_u64(&mut self, v: u64) { self.0 = fold_mul(self.0 ^ v, K1); }
    #[inline]
    fn write_u8(&mut self, v: u8) { self.write_u64(v as u64 | 0x100); }
    #[inline]
    fn write_u32(&mut self, v: u32) { self.write_u64(v as u64 | 0x1_0000_0000); }
    #[inline]
    fn write_usize(&mut self, v: usize) { self.write_u64(v as u64); }
    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for c in &mut chunks {
            self.write_u64(u64::from_le_bytes(c.try_into().unwrap()));
        }
        let rest = chunks.remainder();
        let mut tail = [0u8; 8];
        tail[..rest.len()].copy_from_slice(rest);
        self.write_u64(u64::from_le_bytes(tail) ^ ((rest.len() as u64) << 56));
    }
    fn finish(&self) -> u64 { fold_mul(self.0, K2 ^ 0x9e37_79b9_7f4a_7c15) }
}
