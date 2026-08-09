use std::collections::{HashMap, VecDeque};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnFingerprint {
    pub calls: Vec<(String, String)>,
    pub result_hashes: Vec<String>,
    pub successful_non_read_effects: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StallKind {
    RepeatCall,
    Oscillation,
    BarrenTurns,
    SearchThrash,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressNotice {
    pub kind: StallKind,
    pub detail: String,
    pub strike: u8,
}

#[derive(Debug, Default)]
pub struct ProgressMonitor {
    window: VecDeque<TurnFingerprint>,
    call_counts: HashMap<(String, String), u32>,
    strikes: u8,
}

impl ProgressMonitor {
    pub fn observe(&mut self, fingerprint: TurnFingerprint) -> Option<ProgressNotice> {
        let repeated = fingerprint.calls.iter().find_map(|call| {
            let count = self.call_counts.entry(call.clone()).or_default();
            *count += 1;
            (*count >= 3).then(|| (call.0.clone(), *count))
        });

        self.window.push_back(fingerprint);
        if self.window.len() > 12 {
            self.window.pop_front();
            self.rebuild_counts();
        }

        if let Some((tool, count)) = repeated {
            self.strikes = self.strikes.saturating_add(1);
            return Some(ProgressNotice {
                kind: StallKind::RepeatCall,
                detail: format!("{tool} was called {count} times with identical arguments"),
                strike: self.strikes,
            });
        }

        if self.has_oscillation() {
            self.strikes = self.strikes.saturating_add(1);
            return Some(ProgressNotice {
                kind: StallKind::Oscillation,
                detail: "tool selection is oscillating between two paths".to_string(),
                strike: self.strikes,
            });
        }

        if self.has_search_thrash() {
            self.strikes = self.strikes.saturating_add(1);
            return Some(ProgressNotice {
                kind: StallKind::SearchThrash,
                detail: "six search-like turns changed queries without producing an effect"
                    .to_string(),
                strike: self.strikes,
            });
        }

        if self.has_barren_run() {
            self.strikes = self.strikes.saturating_add(1);
            return Some(ProgressNotice {
                kind: StallKind::BarrenTurns,
                detail: "four turns produced no effect or novel result".to_string(),
                strike: self.strikes,
            });
        }
        None
    }

    pub fn strikes(&self) -> u8 {
        self.strikes
    }

    fn rebuild_counts(&mut self) {
        self.call_counts.clear();
        for turn in &self.window {
            for call in &turn.calls {
                *self.call_counts.entry(call.clone()).or_default() += 1;
            }
        }
    }

    fn has_oscillation(&self) -> bool {
        let names: Vec<&str> = self
            .window
            .iter()
            .rev()
            .take(6)
            .filter_map(|turn| turn.calls.first().map(|call| call.0.as_str()))
            .collect();
        names.len() == 6
            && names[0] == names[2]
            && names[2] == names[4]
            && names[1] == names[3]
            && names[3] == names[5]
            && names[0] != names[1]
    }

    fn has_barren_run(&self) -> bool {
        let last: Vec<&TurnFingerprint> = self.window.iter().rev().take(4).collect();
        if last.len() != 4 || last.iter().any(|turn| turn.successful_non_read_effects > 0) {
            return false;
        }
        let mut hashes = std::collections::HashSet::new();
        last.iter()
            .flat_map(|turn| turn.result_hashes.iter())
            .any(|hash| !hashes.insert(hash))
    }

    fn has_search_thrash(&self) -> bool {
        let last: Vec<&TurnFingerprint> = self.window.iter().rev().take(6).collect();
        last.len() == 6
            && last
                .iter()
                .all(|turn| turn.successful_non_read_effects == 0)
            && last.iter().all(|turn| {
                turn.calls.first().is_some_and(|(name, _)| {
                    let name = name.to_ascii_lowercase();
                    name.contains("search") || name.contains("find") || name.contains("list")
                })
            })
            && last
                .iter()
                .filter_map(|turn| turn.calls.first())
                .map(|call| &call.1)
                .collect::<std::collections::HashSet<_>>()
                .len()
                >= 4
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(name: &str, args: &str, results: &[&str]) -> TurnFingerprint {
        TurnFingerprint {
            calls: vec![(name.to_string(), args.to_string())],
            result_hashes: results.iter().map(|value| (*value).to_string()).collect(),
            successful_non_read_effects: 0,
        }
    }

    #[test]
    fn identical_call_escalates_mechanically() {
        let mut monitor = ProgressMonitor::default();
        assert!(monitor.observe(turn("lookup", "same", &["one"])).is_none());
        assert!(monitor.observe(turn("lookup", "same", &["two"])).is_none());
        assert_eq!(
            monitor
                .observe(turn("lookup", "same", &["three"]))
                .unwrap()
                .strike,
            1
        );
        assert_eq!(
            monitor
                .observe(turn("lookup", "same", &["four"]))
                .unwrap()
                .strike,
            2
        );
        assert_eq!(
            monitor
                .observe(turn("lookup", "same", &["five"]))
                .unwrap()
                .strike,
            3
        );
    }

    #[test]
    fn oscillation_uses_tool_paths_not_model_prose() {
        let mut monitor = ProgressMonitor::default();
        let mut notice = None;
        for index in 0..6 {
            let name = if index % 2 == 0 { "path_a" } else { "path_b" };
            notice = monitor.observe(turn(
                name,
                &format!("args-{index}"),
                &[&format!("r-{index}")],
            ));
        }
        assert_eq!(notice.unwrap().kind, StallKind::Oscillation);
    }

    #[test]
    fn repeated_barren_results_and_search_thrash_are_distinct() {
        let mut barren = ProgressMonitor::default();
        let mut barren_notice = None;
        for index in 0..4 {
            barren_notice =
                barren.observe(turn("lookup", &format!("args-{index}"), &["unchanged"]));
        }
        assert_eq!(barren_notice.unwrap().kind, StallKind::BarrenTurns);

        let mut search = ProgressMonitor::default();
        let mut search_notice = None;
        for index in 0..6 {
            search_notice = search.observe(turn(
                "search_records",
                &format!("query-{index}"),
                &[&format!("result-{index}")],
            ));
        }
        assert_eq!(search_notice.unwrap().kind, StallKind::SearchThrash);
    }
}
