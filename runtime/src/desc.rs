//! Type descriptors: what the runtime knows about a heap object's type.
//!
//! Every object's header names a descriptor by index into one `Table`, so freeing, copying,
//! equality and display need no type argument. Codegen emits the table as data: every type is
//! `repr(C)` and holds only `u32`s, and lists (a record's slots, an enum's variants, names) are
//! `Slice`s into the table's flat arrays.
//!
//! Invariants, which codegen establishes and nothing re-checks beyond Rust's bounds checks:
//! - `descs[0..6]` are `Unit`, `Int`, `Bool`, `Str`, `Bytes`, `Float`, at the indices below
//!   (each scalar's index is also its tag), so the runtime can make strings without being told
//!   their descriptor.
//! - Every index and `Slice` is in range of its array; a `Name` is UTF-8; `fields` is as long as `slots`.
//! - Display names: a struct's or enum's is its module-qualified key (`web::Point`); a variant's
//!   is bare. The enums named `Option` and `Result` (reserved, so no user type has them) print
//!   their variants without the enum name.
//! - Each generic instance has its own descriptor: `Option<Int>` and `Option<Str>` differ,
//!   because their payload slots do.

use core::mem::{align_of, size_of};
use core::slice;

pub const UNIT: u32 = 0;
pub const INT: u32 = 1;
pub const BOOL: u32 = 2;
pub const STR: u32 = 3;
pub const BYTES: u32 = 4;
pub const FLOAT: u32 = 5;

/// One type. `repr(C, u32)`: a `u32` tag (declaration order) then the fields, padded to 20
/// bytes, 4-aligned.
#[repr(C, u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Desc {
    Unit,
    Int,
    Bool,
    Str,
    Bytes,
    /// An `f64`, held in a slot as its bits.
    Float,
    List {
        elem: u32,
    },
    Map {
        key: u32,
        value: u32,
    },
    /// A struct: its slots' descriptors, in field order. Field `i`'s name is
    /// `Table::fields[slots.start + i]`.
    Record {
        name: Name,
        slots: Slice,
    },
    /// Indexes into `Table::variants`; a variant's number is its position there.
    Enum {
        name: Name,
        variants: Slice,
    },
}

/// Bytes `off..off + len` of `Table::names`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Name {
    pub off: u32,
    pub len: u32,
}

/// Elements `start..start + len` of one of the table's arrays.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slice {
    pub start: u32,
    pub len: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Variant {
    pub name: Name,
    /// The payload slots' descriptors, indexes into `Table::slots`.
    pub slots: Slice,
}

// The sizes codegen lays out.
const _: () = assert!(size_of::<Desc>() == 20 && align_of::<Desc>() == 4 && size_of::<Variant>() == 16);

#[derive(Debug, Clone, Copy)]
pub struct Table<'t> {
    pub descs: &'t [Desc],
    /// Descriptor indexes of record and variant slots.
    pub slots: &'t [u32],
    /// Parallel to `slots`: a record slot's field name; empty for a variant slot.
    pub fields: &'t [Name],
    pub variants: &'t [Variant],
    pub names: &'t [u8],
}

impl<'t> Table<'t> {
    pub fn desc(&self, i: u32) -> Desc {
        self.descs[i as usize]
    }

    /// Whether a value of descriptor `i` is a pointer to a heap object.
    pub fn is_ptr(&self, i: u32) -> bool {
        !matches!(self.desc(i), Desc::Unit | Desc::Int | Desc::Bool | Desc::Float)
    }

    pub fn slots(&self, s: Slice) -> &'t [u32] {
        &self.slots[s.start as usize..][..s.len as usize]
    }

    /// Variant `v` of an enum whose variants are `s`.
    pub fn variant(&self, s: Slice, v: u32) -> Variant {
        self.variants[s.start as usize..][..s.len as usize][v as usize]
    }

    /// The name of field `i` of a record whose slots are `s`.
    pub fn field_name(&self, s: Slice, i: usize) -> &'t [u8] {
        self.name(self.fields[s.start as usize + i])
    }

    pub fn name(&self, n: Name) -> &'t [u8] {
        &self.names[n.off as usize..][..n.len as usize]
    }
}

/// `Table`'s arrays as codegen lays them out in linear memory: each array's address and
/// element count.
#[repr(C)]
pub struct RawTable {
    descs: [u32; 2],
    slots: [u32; 2],
    fields: [u32; 2],
    variants: [u32; 2],
    names: [u32; 2],
}

impl RawTable {
    /// The table whose `RawTable` is at `addr`, in a memory whose address 0 is at `base` (0 in
    /// Wasm, a buffer's address in native tests).
    ///
    /// # Safety
    /// A `RawTable` is at `addr`, and each of its arrays is where it says: aligned, never
    /// written, and keeping the invariants above.
    pub unsafe fn read(base: usize, addr: u32) -> Table<'static> {
        // SAFETY: the caller's guarantee.
        unsafe {
            let raw = &*at::<RawTable>(base, addr);
            Table {
                descs: array(base, raw.descs),
                slots: array(base, raw.slots),
                fields: array(base, raw.fields),
                variants: array(base, raw.variants),
                names: array(base, raw.names),
            }
        }
    }
}

fn at<T>(base: usize, addr: u32) -> *const T {
    core::ptr::with_exposed_provenance(base + addr as usize)
}

/// # Safety
/// `len` `T`s at `addr`, aligned, never written.
unsafe fn array<T>(base: usize, [addr, len]: [u32; 2]) -> &'static [T] {
    // SAFETY: the caller's guarantee.
    unsafe { slice::from_raw_parts(at(base, addr), len as usize) }
}
