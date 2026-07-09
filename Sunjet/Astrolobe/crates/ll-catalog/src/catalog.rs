//! The catalog: tables (with typed columns) and the files per table, with manual atomic
//! file persistence. Fully cached in memory (the spec's model); the redb backing is a
//! later swap.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;

use crate::schema::{ColumnDef, ColumnKind, FileRef, TableDef};

const MAGIC: u32 = u32::from_le_bytes(*b"VCAT");
const VERSION: u16 = 1;

/// Catalog operation errors.
#[derive(Debug, PartialEq, Eq)]
pub enum CatalogError {
    TableExists(String),
    NotFound(String),
}

impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CatalogError::TableExists(n) => write!(f, "table '{n}' already exists"),
            CatalogError::NotFound(n) => write!(f, "not found: {n}"),
        }
    }
}
impl std::error::Error for CatalogError {}

/// The in-memory catalog.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Catalog {
    tables: BTreeMap<u32, TableDef>,
    name_to_id: BTreeMap<String, u32>,
    next_table_id: u32,
}

impl Catalog {
    pub fn new() -> Self {
        Catalog {
            tables: BTreeMap::new(),
            name_to_id: BTreeMap::new(),
            next_table_id: 1,
        }
    }

    /// Create a table. Column ids are assigned `1..=n` in declaration order.
    pub fn create_table(
        &mut self,
        name: &str,
        columns: &[(&str, ColumnKind)],
    ) -> Result<u32, CatalogError> {
        if self.name_to_id.contains_key(name) {
            return Err(CatalogError::TableExists(name.to_string()));
        }
        let table_id = self.next_table_id;
        self.next_table_id += 1;
        let columns = columns
            .iter()
            .enumerate()
            .map(|(i, (n, k))| ColumnDef {
                column_id: (i + 1) as u32,
                name: (*n).to_string(),
                kind: *k,
            })
            .collect();
        self.tables.insert(
            table_id,
            TableDef {
                table_id,
                name: name.to_string(),
                columns,
                files: Vec::new(),
            },
        );
        self.name_to_id.insert(name.to_string(), table_id);
        Ok(table_id)
    }

    pub fn table(&self, name: &str) -> Option<&TableDef> {
        self.name_to_id.get(name).and_then(|id| self.tables.get(id))
    }
    pub fn table_by_id(&self, id: u32) -> Option<&TableDef> {
        self.tables.get(&id)
    }
    pub fn tables(&self) -> impl Iterator<Item = &TableDef> {
        self.tables.values()
    }

    /// Register a flushed file under a table.
    pub fn register_file(&mut self, table_id: u32, file: FileRef) -> Result<(), CatalogError> {
        let t = self
            .tables
            .get_mut(&table_id)
            .ok_or_else(|| CatalogError::NotFound(format!("table {table_id}")))?;
        t.files.push(file);
        Ok(())
    }

    pub fn files(&self, table_id: u32) -> &[FileRef] {
        self.tables.get(&table_id).map(|t| t.files.as_slice()).unwrap_or(&[])
    }

    // ---- persistence ----

    /// Atomically persist the catalog to `path` (temp-then-rename, fsync).
    pub fn save<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
        let bytes = self.encode();
        let path = path.as_ref();
        let mut tmp = path.to_path_buf().into_os_string();
        tmp.push(".tmp");
        let tmp = std::path::PathBuf::from(tmp);
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(&bytes)?;
            f.sync_all()?;
        }
        fs::rename(&tmp, path)?;
        // fsync the parent directory so the rename itself survives a crash.
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            if let Ok(d) = fs::File::open(dir) {
                let _ = d.sync_all();
            }
        }
        Ok(())
    }

    /// Load a catalog from `path`.
    pub fn load<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let mut bytes = Vec::new();
        fs::File::open(path)?.read_to_end(&mut bytes)?;
        Self::decode(&bytes)
    }

    fn encode(&self) -> Vec<u8> {
        let mut w = Vec::new();
        w.extend_from_slice(&MAGIC.to_le_bytes());
        w.extend_from_slice(&VERSION.to_le_bytes());
        w.extend_from_slice(&self.next_table_id.to_le_bytes());
        w.extend_from_slice(&(self.tables.len() as u32).to_le_bytes());
        for t in self.tables.values() {
            w.extend_from_slice(&t.table_id.to_le_bytes());
            put_lp(&mut w, &t.name);
            w.extend_from_slice(&(t.columns.len() as u32).to_le_bytes());
            for c in &t.columns {
                w.extend_from_slice(&c.column_id.to_le_bytes());
                put_lp(&mut w, &c.name);
                w.push(c.kind.tag());
                w.extend_from_slice(&c.kind.dim().to_le_bytes());
            }
            w.extend_from_slice(&(t.files.len() as u32).to_le_bytes());
            for f in &t.files {
                put_lp(&mut w, &f.path);
                w.extend_from_slice(&f.min_lsn.to_le_bytes());
                w.extend_from_slice(&f.max_lsn.to_le_bytes());
                w.extend_from_slice(&f.row_count.to_le_bytes());
            }
        }
        w
    }

    fn decode(b: &[u8]) -> io::Result<Self> {
        let mut r = Cur { b, p: 0 };
        if r.u32()? != MAGIC {
            return Err(bad("bad catalog magic"));
        }
        if r.u16()? != VERSION {
            return Err(bad("unsupported catalog version"));
        }
        let next_table_id = r.u32()?;
        let table_count = r.u32()?;
        let mut tables = BTreeMap::new();
        let mut name_to_id = BTreeMap::new();
        for _ in 0..table_count {
            let table_id = r.u32()?;
            let name = r.lp()?;
            let col_count = r.u32()?;
            let mut columns = Vec::with_capacity(col_count as usize);
            for _ in 0..col_count {
                let column_id = r.u32()?;
                let cname = r.lp()?;
                let tag = r.u8()?;
                let dim = r.u16()?;
                let kind = ColumnKind::from_parts(tag, dim).ok_or_else(|| bad("bad column kind"))?;
                columns.push(ColumnDef {
                    column_id,
                    name: cname,
                    kind,
                });
            }
            let file_count = r.u32()?;
            let mut files = Vec::with_capacity(file_count as usize);
            for _ in 0..file_count {
                let path = r.lp()?;
                let min_lsn = r.u64()?;
                let max_lsn = r.u64()?;
                let row_count = r.u64()?;
                files.push(FileRef {
                    path,
                    min_lsn,
                    max_lsn,
                    row_count,
                });
            }
            name_to_id.insert(name.clone(), table_id);
            tables.insert(
                table_id,
                TableDef {
                    table_id,
                    name,
                    columns,
                    files,
                },
            );
        }
        Ok(Catalog {
            tables,
            name_to_id,
            next_table_id,
        })
    }
}

fn put_lp(w: &mut Vec<u8>, s: &str) {
    w.extend_from_slice(&(s.len() as u32).to_le_bytes());
    w.extend_from_slice(s.as_bytes());
}

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

struct Cur<'a> {
    b: &'a [u8],
    p: usize,
}
impl Cur<'_> {
    fn take(&mut self, n: usize) -> io::Result<&[u8]> {
        let end = self.p.checked_add(n).ok_or_else(|| bad("overflow"))?;
        let s = self.b.get(self.p..end).ok_or_else(|| bad("truncated catalog"))?;
        self.p = end;
        Ok(s)
    }
    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> io::Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> io::Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn lp(&mut self) -> io::Result<String> {
        let n = self.u32()? as usize;
        let s = self.take(n)?.to_vec();
        String::from_utf8(s).map_err(|_| bad("invalid utf-8 in catalog"))
    }
}
