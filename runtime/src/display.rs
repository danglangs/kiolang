//! Display: a value as `kio run` prints it, matching the interpreter's `Program::display`.
//!
//! A top-level `Str` is bare; everything else is its source form, with nested strings quoted
//! as Rust's `{:?}`, `Bytes` as `<N bytes>`, a struct as `Name { f: v }` (no braces when it has
//! no fields), an enum as `Name::Variant(a, b)`, and `Option` and `Result` as `Some(x)`, `None`,
//! `Ok(x)`, `Err(x)`, and a function as `<fn f>` or `<closure>`. A work stack, not recursion,
//! so a deep value costs heap, not stack.

use crate::desc::{self, Desc, Table};
use crate::obj::{self, field, from_slot, Header, Key, Map, HEADER};
use alloc::vec::Vec;
use core::fmt::Write;

/// What is left to print.
enum Item<'a> {
    /// A value of descriptor `.0`, held as slot `.1`.
    Val(u32, u64),
    Text(&'a [u8]),
    /// A string, quoted and escaped.
    Quoted(&'a [u8]),
}

/// A fresh `Str` showing the value `slot` of descriptor `desc`; `slot` is not consumed.
///
/// # Safety
/// `slot` holds a value of `desc`, keeping the module's contract (`obj`) for `t`.
pub unsafe fn display(t: &Table, desc: u32, slot: u64) -> *mut Header {
    let mut out = Vec::new();
    // SAFETY: the caller's guarantee.
    unsafe {
        if desc == desc::STR {
            out.extend_from_slice(obj::bytes(from_slot(slot)));
        } else {
            write_value(t, desc, slot, &mut out);
        }
    }
    obj::new_bytes(desc::STR, &out)
}

/// `main`'s value as `run` reports it: whether it failed (a `Result` that is `Err`), and a
/// fresh `Str` showing the `Err`'s payload alone if so (as the interpreter path does, so
/// `Err("bad")` reads `bad`), else the value. `slot` is not consumed.
///
/// # Safety
/// As `display`.
pub unsafe fn outcome(t: &Table, desc: u32, slot: u64) -> (*mut Header, bool) {
    if let Desc::Enum { name, .. } = t.desc(desc) {
        let p = from_slot(slot);
        // SAFETY: the caller's guarantee; an enum has its variant at HEADER, and a `Result`'s
        // `Err` has one slot.
        unsafe {
            if t.name(name) == b"Result" && *field::<u32>(p, HEADER) == 1 {
                let (_, ds) = obj::record(t, p);
                return (display(t, ds[0], *obj::slot(t, p, 0)), true);
            }
        }
    }
    // SAFETY: the caller's guarantee.
    (unsafe { display(t, desc, slot) }, false)
}

/// # Safety
/// As `display`.
unsafe fn write_value(t: &Table, desc: u32, slot: u64, out: &mut Vec<u8>) {
    let mut work = alloc::vec![Item::Val(desc, slot)];
    while let Some(item) = work.pop() {
        match item {
            Item::Text(s) => out.extend_from_slice(s),
            Item::Quoted(s) => quote(s, out),
            // SAFETY: the caller's guarantee covers every value reached from the root.
            Item::Val(d, v) => unsafe { expand(t, d, v, out, &mut work) },
        }
    }
}

/// Prints a leaf, or pushes what a container prints, last first.
///
/// # Safety
/// As `display`.
unsafe fn expand<'a>(t: &Table<'a>, d: u32, v: u64, out: &mut Vec<u8>, work: &mut Vec<Item<'a>>) {
    let mut write = |args: core::fmt::Arguments| Utf8(out).write_fmt(args).unwrap();
    // SAFETY: `v` is a value of `d`; a pointer is a live object laid out as `d` says.
    unsafe {
        let p = from_slot(v);
        match t.desc(d) {
            Desc::Unit => write(format_args!("()")),
            Desc::Int => write(format_args!("{}", v as i64)),
            Desc::Bool => write(format_args!("{}", v != 0)),
            Desc::Float => write(format_args!("{}", f64::from_bits(v))),
            Desc::Str => quote(obj::bytes(p), out),
            Desc::Bytes => write(format_args!("<{} bytes>", obj::bytes(p).len())),
            Desc::List { elem } => {
                work.push(Item::Text(b"]"));
                let items = obj::elems(p).iter().map(|&e| (elem, e));
                separated(work, items.rev(), |work, (d, e)| work.push(Item::Val(d, e)));
                work.push(Item::Text(b"["));
            }
            Desc::Map { key, value } => {
                work.push(Item::Text(b"}"));
                let entries = (*field::<Map>(p, HEADER)).iter().rev();
                separated(work, entries, |work, (k, &e)| {
                    work.push(Item::Val(value, e));
                    work.push(Item::Text(b": "));
                    work.push(match k {
                        Key::Int(n) => Item::Val(key, *n as u64),
                        Key::Str(s) => Item::Quoted(s),
                    });
                });
                work.push(Item::Text(b"{"));
            }
            Desc::Record { name, slots } => {
                let (off, ds) = obj::record(t, p);
                if !ds.is_empty() {
                    work.push(Item::Text(b" }"));
                    let fields = (0..ds.len()).rev();
                    separated(work, fields, |work, i| {
                        work.push(Item::Val(ds[i], *field(p, off + 8 * i)));
                        work.push(Item::Text(b": "));
                        work.push(Item::Text(t.field_name(slots, i)));
                    });
                    work.push(Item::Text(b" { "));
                }
                work.push(Item::Text(t.name(name)));
            }
            Desc::Enum { name, variants } => {
                let (off, ds) = obj::record(t, p);
                let variant = t.variant(variants, *field(p, HEADER));
                if !ds.is_empty() {
                    work.push(Item::Text(b")"));
                    let fields = (0..ds.len()).rev();
                    separated(work, fields, |work, i| {
                        work.push(Item::Val(ds[i], *field(p, off + 8 * i)))
                    });
                    work.push(Item::Text(b"("));
                }
                work.push(Item::Text(t.name(variant.name)));
                if !matches!(t.name(name), b"Option" | b"Result") {
                    work.push(Item::Text(b"::"));
                    work.push(Item::Text(t.name(name)));
                }
            }
            // `d` may be a `fn` type's, which has no name: the object's own has it.
            Desc::Fn { .. } => {
                let Desc::Fn { name, .. } = t.desc((*p).desc) else {
                    panic!("a fn value that is not a function object")
                };
                work.push(Item::Text(t.name(name)));
            }
        }
    }
}

/// Pushes `items` (given last first) with `, ` between them, so they pop in order.
fn separated<'a, I>(
    work: &mut Vec<Item<'a>>,
    items: impl Iterator<Item = I>,
    mut push: impl FnMut(&mut Vec<Item<'a>>, I),
) {
    for (n, item) in items.enumerate() {
        if n > 0 {
            work.push(Item::Text(b", "));
        }
        push(work, item);
    }
}

/// `s` as Rust's `{:?}` prints a `str`, which is what the interpreter does.
fn quote(s: &[u8], out: &mut Vec<u8>) {
    let s = core::str::from_utf8(s).expect("a Str is UTF-8");
    write!(Utf8(out), "{s:?}").unwrap();
}

/// `fmt::Write` onto bytes.
struct Utf8<'a>(&'a mut Vec<u8>);

impl Write for Utf8<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.0.extend_from_slice(s.as_bytes());
        Ok(())
    }
}
