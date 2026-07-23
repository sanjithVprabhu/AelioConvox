//! Minimal structure path: `a.b.c`, `a[0]`, `a[*]`, `a["x.y"]`.
//! No filters, no expressions, no recursive descent.

use crate::types::{AelioError, AelioResult, ReasonCode, Value};
use indexmap::IndexMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathSeg {
    Key(String),
    Index(usize),
    Wildcard,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Path {
    pub segs: Vec<PathSeg>,
}

impl Path {
    pub fn parse(s: &str) -> AelioResult<Self> {
        let s = s
            .strip_prefix("$.")
            .unwrap_or(s.strip_prefix('$').unwrap_or(s));
        if s.is_empty() {
            return Ok(Path { segs: vec![] });
        }
        let mut segs = Vec::new();
        let mut rest = s;
        while !rest.is_empty() {
            if rest.starts_with('[') {
                let end = rest
                    .find(']')
                    .ok_or_else(|| AelioError::new(ReasonCode::ParseError, "unclosed [ in path"))?;
                let inner = &rest[1..end];
                if inner == "*" {
                    segs.push(PathSeg::Wildcard);
                } else if let Ok(i) = inner.parse::<usize>() {
                    segs.push(PathSeg::Index(i));
                } else if inner.starts_with('"') && inner.ends_with('"') && inner.len() >= 2 {
                    segs.push(PathSeg::Key(inner[1..inner.len() - 1].to_string()));
                } else {
                    return Err(AelioError::new(
                        ReasonCode::ParseError,
                        format!("bad path index: {inner}"),
                    ));
                }
                rest = &rest[end + 1..];
                if rest.starts_with('.') {
                    rest = &rest[1..];
                }
            } else {
                let end = rest.find(['.', '[']).unwrap_or(rest.len());
                let key = &rest[..end];
                if key.is_empty() {
                    return Err(AelioError::new(ReasonCode::ParseError, "empty path key"));
                }
                segs.push(PathSeg::Key(key.to_string()));
                rest = &rest[end..];
                if rest.starts_with('.') {
                    rest = &rest[1..];
                }
            }
        }
        Ok(Path { segs })
    }
}

pub fn get_path(doc: &Value, path: &str) -> AelioResult<Option<Value>> {
    let p = Path::parse(path)?;
    Ok(get_path_segs(doc, &p.segs))
}

fn get_path_segs(doc: &Value, segs: &[PathSeg]) -> Option<Value> {
    if segs.is_empty() {
        return Some(doc.clone());
    }
    match (&segs[0], doc) {
        (PathSeg::Key(k), Value::Map(m)) => get_path_segs(m.get(k)?, &segs[1..]),
        (PathSeg::Index(i), Value::List(l)) => get_path_segs(l.get(*i)?, &segs[1..]),
        (PathSeg::Wildcard, Value::List(l)) => {
            // Return first match for single get; use get_path_all for all.
            for item in l {
                if let Some(v) = get_path_segs(item, &segs[1..]) {
                    return Some(v);
                }
            }
            None
        }
        _ => None,
    }
}

pub fn get_path_all(doc: &Value, path: &str) -> AelioResult<Vec<Value>> {
    let p = Path::parse(path)?;
    let mut out = Vec::new();
    collect(doc, &p.segs, &mut out);
    Ok(out)
}

fn collect(doc: &Value, segs: &[PathSeg], out: &mut Vec<Value>) {
    if segs.is_empty() {
        out.push(doc.clone());
        return;
    }
    match (&segs[0], doc) {
        (PathSeg::Key(k), Value::Map(m)) => {
            if let Some(v) = m.get(k) {
                collect(v, &segs[1..], out);
            }
        }
        (PathSeg::Index(i), Value::List(l)) => {
            if let Some(v) = l.get(*i) {
                collect(v, &segs[1..], out);
            }
        }
        (PathSeg::Wildcard, Value::List(l)) => {
            for item in l {
                collect(item, &segs[1..], out);
            }
        }
        _ => {}
    }
}

pub fn set_path(doc: &Value, path: &str, value: Value) -> AelioResult<Value> {
    let p = Path::parse(path)?;
    if p.segs.is_empty() {
        return Ok(value);
    }
    set_segs(doc, &p.segs, value)
}

fn set_segs(doc: &Value, segs: &[PathSeg], value: Value) -> AelioResult<Value> {
    if segs.len() == 1 {
        match &segs[0] {
            PathSeg::Key(k) => {
                let mut m = match doc {
                    Value::Map(m) => m.clone(),
                    Value::Null => IndexMap::new(),
                    _ => {
                        return Err(AelioError::new(
                            ReasonCode::TypeViolation,
                            "set_path expects map at key",
                        ))
                    }
                };
                m.insert(k.clone(), value);
                Ok(Value::Map(m))
            }
            PathSeg::Index(i) => {
                let mut l = match doc {
                    Value::List(l) => l.clone(),
                    Value::Null => Vec::new(),
                    _ => {
                        return Err(AelioError::new(
                            ReasonCode::TypeViolation,
                            "set_path expects list at index",
                        ))
                    }
                };
                if *i >= l.len() {
                    l.resize(*i + 1, Value::Null);
                }
                l[*i] = value;
                Ok(Value::List(l))
            }
            PathSeg::Wildcard => Err(AelioError::new(
                ReasonCode::Validation,
                "cannot set via wildcard",
            )),
        }
    } else {
        match &segs[0] {
            PathSeg::Key(k) => {
                let mut m = match doc {
                    Value::Map(m) => m.clone(),
                    Value::Null => IndexMap::new(),
                    _ => {
                        return Err(AelioError::new(
                            ReasonCode::TypeViolation,
                            "set_path expects map",
                        ))
                    }
                };
                let child = m.get(k).cloned().unwrap_or(Value::Null);
                let next = set_segs(&child, &segs[1..], value)?;
                m.insert(k.clone(), next);
                Ok(Value::Map(m))
            }
            PathSeg::Index(i) => {
                let mut l = match doc {
                    Value::List(l) => l.clone(),
                    Value::Null => Vec::new(),
                    _ => {
                        return Err(AelioError::new(
                            ReasonCode::TypeViolation,
                            "set_path expects list",
                        ))
                    }
                };
                if *i >= l.len() {
                    l.resize(*i + 1, Value::Null);
                }
                let next = set_segs(&l[*i], &segs[1..], value)?;
                l[*i] = next;
                Ok(Value::List(l))
            }
            PathSeg::Wildcard => Err(AelioError::new(
                ReasonCode::Validation,
                "cannot set via wildcard",
            )),
        }
    }
}

pub fn has_path(doc: &Value, path: &str) -> AelioResult<bool> {
    Ok(get_path(doc, path)?.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::indexmap;

    #[test]
    fn get_nested() {
        let doc = Value::Map(indexmap! {
            "a".into() => Value::Map(indexmap! {
                "b".into() => Value::List(vec![Value::Int(1), Value::Int(2)]),
            }),
        });
        assert_eq!(get_path(&doc, "a.b[1]").unwrap(), Some(Value::Int(2)));
        assert_eq!(get_path(&doc, "$.a.b[0]").unwrap(), Some(Value::Int(1)));
    }

    #[test]
    fn set_creates_path() {
        let doc = Value::Null;
        let next = set_path(&doc, "user.phone", Value::str("+91")).unwrap();
        assert_eq!(
            get_path(&next, "user.phone").unwrap(),
            Some(Value::str("+91"))
        );
    }
}
