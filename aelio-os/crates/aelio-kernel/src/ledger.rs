//! Turn ledger (§12.2, App G): append-only per flow instance; `seq` monotonic + gapless;
//! per-instance BLAKE3 hash chain (tamper-evident, §28). Replay consumes entries in `seq` order
//! (§12.3): INJECT = write payload in; VERIFY = recompute + compare, mismatch ⇒ hard refuse.

use aelio_sol::{value_hash, SolValue};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// Replay consumes the payload instead of acting (LLM output, tool result, now/uuid, …).
    Inject,
    /// Replay recomputes deterministically and compares; mismatch ⇒ hard refuse.
    Verify,
    /// Neither consumed nor verified for determinism (informational / init).
    Info,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub seq: u64,
    pub turn_id: String,
    pub nid: Option<String>,
    pub kind: String,
    pub category: Category,
    pub payload: SolValue,
    pub payload_hash: String,
    /// Chain over the previous entry (envelope+payload); empty for seq 0.
    pub prev: String,
}

impl Entry {
    pub fn to_sol(&self) -> SolValue {
        SolValue::map([
            ("seq", SolValue::Int(self.seq as i64)),
            ("turn_id", SolValue::str(self.turn_id.clone())),
            (
                "nid",
                self.nid
                    .clone()
                    .map(SolValue::Str)
                    .unwrap_or(SolValue::Null),
            ),
            ("kind", SolValue::str(self.kind.clone())),
            (
                "category",
                SolValue::str(match self.category {
                    Category::Inject => "inject",
                    Category::Verify => "verify",
                    Category::Info => "info",
                }),
            ),
            ("payload", self.payload.clone()),
            ("payload_hash", SolValue::str(self.payload_hash.clone())),
            ("prev", SolValue::str(self.prev.clone())),
        ])
    }

    pub fn from_sol(value: &SolValue) -> Result<Self, String> {
        let map = value.as_map().ok_or("ledger entry must be a map")?;
        let allowed = [
            "seq",
            "turn_id",
            "nid",
            "kind",
            "category",
            "payload",
            "payload_hash",
            "prev",
        ];
        if map.len() != allowed.len() {
            return Err("invalid durable ledger field count".into());
        }
        if let Some(key) = map.keys().find(|key| !allowed.contains(&key.as_str())) {
            return Err(format!("unknown durable ledger field `{key}`"));
        }
        let seq = match map.get("seq") {
            Some(SolValue::Int(value)) => {
                u64::try_from(*value).map_err(|_| "ledger seq must be non-negative")?
            }
            _ => return Err("ledger seq missing".into()),
        };
        let text = |key: &str| match map.get(key) {
            Some(SolValue::Str(value)) => Ok(value.clone()),
            _ => Err(format!("ledger `{key}` missing")),
        };
        let category = match text("category")?.as_str() {
            "inject" => Category::Inject,
            "verify" => Category::Verify,
            "info" => Category::Info,
            _ => return Err("bad ledger category".into()),
        };
        let nid = match map.get("nid") {
            Some(SolValue::Null) => None,
            Some(SolValue::Str(value)) => Some(value.clone()),
            _ => return Err("ledger nid must be string|null".into()),
        };
        Ok(Entry {
            seq,
            turn_id: text("turn_id")?,
            nid,
            kind: text("kind")?,
            category,
            payload: map
                .get("payload")
                .cloned()
                .ok_or("ledger payload missing")?,
            payload_hash: text("payload_hash")?,
            prev: text("prev")?,
        })
    }
}

#[derive(Debug, Default, Clone)]
pub struct Ledger {
    entries: Vec<Entry>,
}

impl Ledger {
    pub fn from_entries(entries: Vec<Entry>) -> Result<Self, String> {
        let ledger = Ledger { entries };
        ledger
            .verify_chain()
            .map_err(|seq| format!("ledger integrity failure at seq {seq}"))?;
        Ok(ledger)
    }

    pub fn append(
        &mut self,
        turn_id: &str,
        nid: Option<&str>,
        kind: &str,
        category: Category,
        payload: SolValue,
    ) -> &Entry {
        let seq = self.entries.len() as u64;
        let payload_hash = value_hash(&payload);
        let prev = self.entries.last().map(chain_hash).unwrap_or_default();
        self.entries.push(Entry {
            seq,
            turn_id: turn_id.to_string(),
            nid: nid.map(str::to_string),
            kind: kind.to_string(),
            category,
            payload,
            payload_hash,
            prev,
        });
        self.entries.last().unwrap()
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        let entries = self
            .entries
            .iter()
            .map(|entry| {
                let payload: serde_json::Value =
                    serde_json::from_str(&aelio_sol::canonical_string(&entry.payload))?;
                Ok(serde_json::json!({
                    "seq": entry.seq,
                    "turn_id": entry.turn_id,
                    "nid": entry.nid,
                    "kind": entry.kind,
                    "category": match entry.category {
                        Category::Inject => "inject",
                        Category::Verify => "verify",
                        Category::Info => "info",
                    },
                    "payload": payload,
                    "payload_hash": entry.payload_hash,
                    "prev": entry.prev,
                }))
            })
            .collect::<Result<Vec<_>, serde_json::Error>>()?;
        serde_json::to_string_pretty(&entries)
    }

    pub fn from_json_str(text: &str) -> Result<Self, String> {
        let values: Vec<serde_json::Value> =
            serde_json::from_str(text).map_err(|error| error.to_string())?;
        let mut entries = Vec::with_capacity(values.len());
        for value in values {
            let object = value.as_object().ok_or("ledger entry must be an object")?;
            let allowed = [
                "seq",
                "turn_id",
                "nid",
                "kind",
                "category",
                "payload",
                "payload_hash",
                "prev",
            ];
            if let Some(unknown) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
                return Err(format!("unknown ledger field `{unknown}`"));
            }
            let category = match object.get("category").and_then(serde_json::Value::as_str) {
                Some("inject") => Category::Inject,
                Some("verify") => Category::Verify,
                Some("info") => Category::Info,
                _ => return Err("bad ledger category".into()),
            };
            let payload =
                crate::json::from_json(object.get("payload").ok_or("ledger payload missing")?)
                    .map_err(|error| error.to_string())?;
            entries.push(Entry {
                seq: object
                    .get("seq")
                    .and_then(serde_json::Value::as_u64)
                    .ok_or("ledger seq missing")?,
                turn_id: string_field(object, "turn_id")?,
                nid: match object.get("nid") {
                    None | Some(serde_json::Value::Null) => None,
                    Some(serde_json::Value::String(value)) => Some(value.clone()),
                    _ => return Err("ledger nid must be string|null".into()),
                },
                kind: string_field(object, "kind")?,
                category,
                payload,
                payload_hash: string_field(object, "payload_hash")?,
                prev: string_field(object, "prev")?,
            });
        }
        let ledger = Ledger { entries };
        ledger
            .verify_chain()
            .map_err(|seq| format!("ledger integrity failure at seq {seq}"))?;
        Ok(ledger)
    }

    /// Verify the chain is gapless (seq 0..n) and each `prev` matches — a test, not a comment
    /// (handoff Working rules). Returns the first offending seq on failure.
    pub fn verify_chain(&self) -> Result<(), u64> {
        let mut prev = String::new();
        for (i, e) in self.entries.iter().enumerate() {
            if e.seq != i as u64 {
                return Err(e.seq);
            }
            if e.payload_hash != value_hash(&e.payload) {
                return Err(e.seq);
            }
            if e.prev != prev {
                return Err(e.seq);
            }
            prev = chain_hash(e);
        }
        Ok(())
    }
}

fn string_field(
    object: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<String, String> {
    object
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("ledger `{key}` missing"))
}

fn chain_hash(e: &Entry) -> String {
    // Hash the envelope identity + payload_hash + prev to chain (App G `prev` intent).
    let env = SolValue::map([
        ("seq", SolValue::Int(e.seq as i64)),
        ("turn_id", SolValue::str(e.turn_id.clone())),
        (
            "nid",
            e.nid.clone().map(SolValue::Str).unwrap_or(SolValue::Null),
        ),
        ("kind", SolValue::str(e.kind.clone())),
        ("payload_hash", SolValue::str(e.payload_hash.clone())),
        ("prev", SolValue::str(e.prev.clone())),
    ]);
    value_hash(&env)
}
