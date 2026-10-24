use std::marker::PhantomData;

use rusqlite::ToSql;
use uuid::Uuid;

/// A synced table. `build.rs` makes one unit struct per table in
/// kron.sqlite, as `Facts` for `facts`, with its columns in a module named
/// after it: `facts::TEXT`.
pub trait Table: Copy {
    const NAME: &'static str;
    /// Its primary key columns, in key order.
    const KEY: &'static [&'static str];
    /// The key's column types as a tuple, `(Blob,)` or `(Blob, Text)`.
    type Key;
}

/// A table with `deleted_at`: only these delete.
pub trait SoftDelete: Table {}

/// A column of `T` holding values of type `V`. Key columns have none, so a
/// key can't be changed.
pub struct Column<T, V> {
    name: &'static str,
    _types: PhantomData<fn() -> (T, V)>,
}

// Derived, these would ask `T` and `V` to be `Clone` too.
impl<T, V> Clone for Column<T, V> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T, V> Copy for Column<T, V> {}

impl<T, V> Column<T, V> {
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            _types: PhantomData,
        }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    /// The column with a value, for `Write::insert`.
    pub fn value<'a>(self, value: &'a impl Fits<V>) -> Assignment<'a, T> {
        Assignment {
            column: self.name,
            value,
            _table: PhantomData,
        }
    }
}

/// A column of `T` and its value, for `Write::insert`.
pub struct Assignment<'a, T> {
    pub(super) column: &'static str,
    pub(super) value: &'a dyn ToSql,
    _table: PhantomData<fn() -> T>,
}

/// SQLite's types, as a STRICT column declares them.
pub struct Integer;
pub struct Real;
pub struct Text;
pub struct Blob;
/// A column that may be NULL.
pub struct Nullable<V>(PhantomData<V>);

/// A Rust value a column of type `V` takes.
pub trait Fits<V>: ToSql {}

impl<V, T: Fits<V> + ?Sized> Fits<V> for &T {}
impl<V, T: Fits<V>> Fits<Nullable<V>> for Option<T> {}

impl Fits<Text> for str {}
impl Fits<Text> for String {}

impl Fits<Integer> for i64 {}
impl Fits<Integer> for i32 {}
impl Fits<Integer> for u32 {}
impl Fits<Integer> for i16 {}
impl Fits<Integer> for u16 {}
impl Fits<Integer> for i8 {}
impl Fits<Integer> for u8 {}
impl Fits<Integer> for bool {}

impl Fits<Real> for f64 {}
impl Fits<Real> for f32 {}

impl Fits<Blob> for Uuid {}
impl<const N: usize> Fits<Blob> for [u8; N] {}
impl Fits<Blob> for [u8] {}
impl Fits<Blob> for Vec<u8> {}

/// A value for each column of a table's key, of type `K`: one value for a
/// key of one column, a tuple for two.
pub trait Key<K> {
    fn values(&self) -> Vec<&dyn ToSql>;
}

impl<V, T: Fits<V>> Key<(V,)> for T {
    fn values(&self) -> Vec<&dyn ToSql> {
        vec![self]
    }
}

impl<V1, V2, T1: Fits<V1>, T2: Fits<V2>> Key<(V1, V2)> for (T1, T2) {
    fn values(&self) -> Vec<&dyn ToSql> {
        vec![&self.0, &self.1]
    }
}

include!(concat!(env!("OUT_DIR"), "/schema.rs"));
