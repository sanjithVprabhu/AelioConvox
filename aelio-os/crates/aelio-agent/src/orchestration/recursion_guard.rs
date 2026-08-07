//! Bounded recursion depth for nested Orchestrate calls.

const DEFAULT_MAX_DEPTH: u8 = 3;

#[derive(Debug, Clone, Copy)]
pub struct RecursionGuard {
    max_depth: u8,
    current: u8,
}

impl RecursionGuard {
    pub fn new(max_depth: Option<u8>) -> Self {
        Self {
            max_depth: max_depth.unwrap_or(DEFAULT_MAX_DEPTH),
            current: 0,
        }
    }

    pub fn enter(&mut self) -> Result<u8, u8> {
        if self.current >= self.max_depth {
            return Err(self.current);
        }
        self.current += 1;
        Ok(self.current)
    }

    pub fn exit(&mut self) {
        self.current = self.current.saturating_sub(1);
    }

    pub fn depth(&self) -> u8 {
        self.current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_beyond_max_depth() {
        let mut guard = RecursionGuard::new(Some(2));
        assert!(guard.enter().is_ok());
        assert!(guard.enter().is_ok());
        assert!(guard.enter().is_err());
    }
}
