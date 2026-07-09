//! On-disk Text section: serialize an [`InvertedIndex`] and search over the bytes via
//! [`TextView`] without rebuilding. Embeds in a `.vss` file as an opaque `Text` section.
//!
//! Layout (little-endian, section-relative offsets):
//! ```text
//! header(40B): column_id u32, num_docs u32, num_terms u32, total_tokens u64,
//!              doclen_off u32, term_off_off u32, term_meta_off u32,
//!              term_data_off u32, postings_off u32
//! doc_len:     u32[num_docs]
//! term_offs:   u32[num_terms+1]            // into term_data (sorted terms)
//! term_meta:   (posting_off u32, df u32)[num_terms]
//! term_data:   concatenated sorted UTF-8 term bytes
//! postings:    per term, df × (delta_doc uvarint, tf uvarint)
//! ```

use std::collections::HashMap;

use crate::index::{bm25_idf, bm25_term, top_k, InvertedIndex};
use crate::tokenize::tokenize;

const HEADER_LEN: usize = 40;

#[derive(Debug)]
pub enum TextError {
    Truncated(&'static str),
    Bad(String),
}

/// Serialize an inverted index to Text section bytes.
pub fn serialize_text_index(idx: &InvertedIndex, column_id: u32) -> Vec<u8> {
    let num_docs = idx.num_docs();
    // Terms in sorted order (BTreeMap iteration order).
    let terms: Vec<(&str, &[(u32, u32)])> = idx.iter_terms().collect();
    let num_terms = terms.len();

    // Build regions.
    let mut doc_len_bytes = Vec::with_capacity(num_docs * 4);
    for d in 0..num_docs {
        doc_len_bytes.extend_from_slice(&idx.doc_len(d as u32).to_le_bytes());
    }

    let mut term_data = Vec::new();
    let mut term_offs: Vec<u32> = Vec::with_capacity(num_terms + 1);
    term_offs.push(0);
    for (t, _) in &terms {
        term_data.extend_from_slice(t.as_bytes());
        term_offs.push(term_data.len() as u32);
    }

    let mut postings_bytes = Vec::new();
    let mut term_meta: Vec<(u32, u32)> = Vec::with_capacity(num_terms); // (posting_off, df)
    for (_, posting) in &terms {
        let posting_off = postings_bytes.len() as u32;
        term_meta.push((posting_off, posting.len() as u32));
        let mut prev = 0u32;
        for &(doc, tf) in *posting {
            write_uvarint(&mut postings_bytes, (doc - prev) as u64);
            write_uvarint(&mut postings_bytes, tf as u64);
            prev = doc;
        }
    }

    // Region offsets (section-relative).
    let doclen_off = HEADER_LEN as u32;
    let term_off_off = doclen_off + doc_len_bytes.len() as u32;
    let term_meta_off = term_off_off + (term_offs.len() * 4) as u32;
    let term_data_off = term_meta_off + (term_meta.len() * 8) as u32;
    let postings_off = term_data_off + term_data.len() as u32;

    let mut out = Vec::with_capacity(postings_off as usize + postings_bytes.len());
    out.extend_from_slice(&column_id.to_le_bytes());
    out.extend_from_slice(&(num_docs as u32).to_le_bytes());
    out.extend_from_slice(&(num_terms as u32).to_le_bytes());
    out.extend_from_slice(&idx.total_tokens().to_le_bytes());
    out.extend_from_slice(&doclen_off.to_le_bytes());
    out.extend_from_slice(&term_off_off.to_le_bytes());
    out.extend_from_slice(&term_meta_off.to_le_bytes());
    out.extend_from_slice(&term_data_off.to_le_bytes());
    out.extend_from_slice(&postings_off.to_le_bytes());
    debug_assert_eq!(out.len(), HEADER_LEN);

    out.extend_from_slice(&doc_len_bytes);
    for o in &term_offs {
        out.extend_from_slice(&o.to_le_bytes());
    }
    for (po, df) in &term_meta {
        out.extend_from_slice(&po.to_le_bytes());
        out.extend_from_slice(&df.to_le_bytes());
    }
    out.extend_from_slice(&term_data);
    out.extend_from_slice(&postings_bytes);
    out
}

/// A read-only view over serialized Text section bytes.
pub struct TextView<'a> {
    bytes: &'a [u8],
    num_docs: usize,
    num_terms: usize,
    total_tokens: u64,
    doclen_off: usize,
    term_off_off: usize,
    term_meta_off: usize,
    term_data_off: usize,
    postings_off: usize,
}

impl<'a> TextView<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, TextError> {
        if bytes.len() < HEADER_LEN {
            return Err(TextError::Truncated("header"));
        }
        let view = TextView {
            bytes,
            num_docs: rd_u32(bytes, 4) as usize,
            num_terms: rd_u32(bytes, 8) as usize,
            total_tokens: rd_u64(bytes, 12),
            doclen_off: rd_u32(bytes, 20) as usize,
            term_off_off: rd_u32(bytes, 24) as usize,
            term_meta_off: rd_u32(bytes, 28) as usize,
            term_data_off: rd_u32(bytes, 32) as usize,
            postings_off: rd_u32(bytes, 36) as usize,
        };
        // All region offsets must be within bounds so data reads can't run wild.
        let len = bytes.len();
        if view.doclen_off > len
            || view.term_off_off > len
            || view.term_meta_off > len
            || view.term_data_off > len
            || view.postings_off > len
        {
            return Err(TextError::Truncated("regions"));
        }
        Ok(view)
    }

    pub fn num_docs(&self) -> usize {
        self.num_docs
    }
    pub fn num_terms(&self) -> usize {
        self.num_terms
    }

    pub fn doc_len(&self, doc: u32) -> u32 {
        let off = self.doclen_off.saturating_add((doc as usize).saturating_mul(4));
        rd_u32(self.bytes, off)
    }

    fn term_bytes(&self, i: usize) -> &[u8] {
        let a = rd_u32(self.bytes, self.term_off_off.saturating_add(i * 4)) as usize;
        let b = rd_u32(self.bytes, self.term_off_off.saturating_add((i + 1) * 4)) as usize;
        let start = self.term_data_off.saturating_add(a);
        let end = self.term_data_off.saturating_add(b);
        self.bytes.get(start..end).unwrap_or(&[])
    }

    /// Binary search for a term; returns its index in the sorted dictionary.
    fn term_index(&self, term: &str) -> Option<usize> {
        let needle = term.as_bytes();
        let (mut lo, mut hi) = (0usize, self.num_terms);
        while lo < hi {
            let mid = (lo + hi) / 2;
            match self.term_bytes(mid).cmp(needle) {
                std::cmp::Ordering::Equal => return Some(mid),
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
            }
        }
        None
    }

    /// Decode a term's postings (`(doc, tf)`), or empty if the term is absent.
    pub fn postings(&self, term: &str) -> Vec<(u32, u32)> {
        let Some(i) = self.term_index(term) else {
            return Vec::new();
        };
        let meta = self.term_meta_off.saturating_add(i * 8);
        let posting_off = rd_u32(self.bytes, meta) as usize;
        // A term can't appear in more documents than exist — cap to avoid huge allocations
        // from a corrupt doc-frequency.
        let df = (rd_u32(self.bytes, meta + 4) as usize).min(self.num_docs);
        let mut pos = self.postings_off.saturating_add(posting_off);
        let mut out = Vec::with_capacity(df);
        let mut prev = 0u32;
        for _ in 0..df {
            let delta = read_uvarint(self.bytes, &mut pos).unwrap_or(0) as u32;
            let tf = read_uvarint(self.bytes, &mut pos).unwrap_or(0) as u32;
            let doc = prev.wrapping_add(delta);
            out.push((doc, tf));
            prev = doc;
        }
        out
    }

    fn avgdl(&self) -> f32 {
        if self.num_docs == 0 {
            0.0
        } else {
            self.total_tokens as f32 / self.num_docs as f32
        }
    }

    /// BM25 top-`k` for a free-text query (same scoring as the in-memory index).
    pub fn bm25(&self, query: &str, k: usize) -> Vec<(u32, f32)> {
        let avgdl = self.avgdl();
        let mut scores: HashMap<u32, f32> = HashMap::new();
        let mut terms = tokenize(query);
        terms.sort();
        terms.dedup();
        for term in terms {
            let posting = self.postings(&term);
            if posting.is_empty() {
                continue;
            }
            let idf = bm25_idf(self.num_docs as f32, posting.len() as f32);
            for (doc, tf) in posting {
                let dl = self.doc_len(doc) as f32;
                *scores.entry(doc).or_insert(0.0) += bm25_term(idf, tf as f32, dl, avgdl);
            }
        }
        top_k(scores, k)
    }
}

// ---- varint + little-endian helpers ----

fn write_uvarint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v != 0 {
            out.push(b | 0x80);
        } else {
            out.push(b);
            break;
        }
    }
}

fn read_uvarint(b: &[u8], pos: &mut usize) -> Option<u64> {
    let mut shift = 0u32;
    let mut res = 0u64;
    loop {
        let byte = *b.get(*pos)?;
        *pos += 1;
        res |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            return Some(res);
        }
        shift += 7;
    }
}

// Bounds-checked: never panic; 0 on out-of-bounds.
fn rd_u32(b: &[u8], o: usize) -> u32 {
    b.get(o..o + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap())).unwrap_or(0)
}
fn rd_u64(b: &[u8], o: usize) -> u64 {
    b.get(o..o + 8).map(|s| u64::from_le_bytes(s.try_into().unwrap())).unwrap_or(0)
}
