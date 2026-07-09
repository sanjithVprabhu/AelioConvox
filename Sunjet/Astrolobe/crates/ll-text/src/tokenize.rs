//! A minimal Unicode-aware tokenizer: lowercase, split on non-alphanumeric.
//!
//! Deliberately simple for v0. Stemming, stop-words, n-grams, and language-specific
//! analyzers are later analyzer features.

/// Tokenize `text` into lowercase alphanumeric terms.
pub fn tokenize(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            cur.extend(ch.to_lowercase());
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::tokenize;

    #[test]
    fn splits_and_lowercases() {
        assert_eq!(tokenize("Hello, World!"), vec!["hello", "world"]);
        assert_eq!(tokenize("derivatives AND volatility"), vec!["derivatives", "and", "volatility"]);
        assert!(tokenize("   ").is_empty());
        assert_eq!(tokenize("a1b2-c3"), vec!["a1b2", "c3"]);
    }
}
