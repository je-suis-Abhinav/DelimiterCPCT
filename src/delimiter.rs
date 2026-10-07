use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DelimiterKind {
    Parenthesis,
    Brace,
    Bracket,
}

impl DelimiterKind {
    pub fn from_open(c: char) -> Option<Self> {
        match c {
            '(' => Some(Self::Parenthesis),
            '{' => Some(Self::Brace),
            '[' => Some(Self::Bracket),
            _ => None,
        }
    }

    pub fn from_close(c: char) -> Option<Self> {
        match c {
            ')' => Some(Self::Parenthesis),
            '}' => Some(Self::Brace),
            ']' => Some(Self::Bracket),
            _ => None,
        }
    }

    pub fn open_char(self) -> char {
        match self {
            Self::Parenthesis => '(',
            Self::Brace => '{',
            Self::Bracket => '[',
        }
    }

    pub fn close_char(self) -> char {
        match self {
            Self::Parenthesis => ')',
            Self::Brace => '}',
            Self::Bracket => ']',
        }
    }
}

impl fmt::Display for DelimiterKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.open_char())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputPosition {
    pub byte_offset: usize,
    pub char_index: usize,
    pub delimiter_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelimiterCheckpoint {
    pub id: usize,
    pub kind: DelimiterKind,
    pub opening_position: InputPosition,
    pub depth: usize,
    pub matched: bool,
    pub closing_position: Option<InputPosition>,
    pub parser_state: Option<String>,
}

impl DelimiterCheckpoint {
    pub fn debug_summary(&self) -> String {
        let closing = self
            .closing_position
            .map(|p| format!("byte={}, char={}", p.byte_offset, p.char_index))
            .unwrap_or_else(|| "none".to_string());
        let parser_state = self.parser_state.as_deref().unwrap_or("not captured");
        format!(
            "Checkpoint #{id}: delimiter={delim}, byte={byte}, char={char_idx}, delimiter_index={delim_idx}, depth={depth}, matched={matched}, closing={closing}, parser_state={parser_state}",
            id = self.id,
            delim = self.kind.open_char(),
            byte = self.opening_position.byte_offset,
            char_idx = self.opening_position.char_index,
            delim_idx = self.opening_position.delimiter_index,
            depth = self.depth,
            matched = self.matched,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelimiterMatch {
    pub checkpoint_id: usize,
    pub kind: DelimiterKind,
    pub opening_position: InputPosition,
    pub closing_position: InputPosition,
    pub depth: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnexpectedClosing {
    pub kind: DelimiterKind,
    pub position: InputPosition,
    pub depth: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MismatchedClosing {
    pub checkpoint_id: usize,
    pub opening_kind: DelimiterKind,
    pub opening_position: InputPosition,
    pub closing_kind: DelimiterKind,
    pub closing_position: InputPosition,
    pub expected_closing: char,
    pub actual_closing: char,
    pub depth: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelimiterReport {
    pub input: String,
    pub checkpoints: Vec<DelimiterCheckpoint>,
    pub matches: Vec<DelimiterMatch>,
    pub unmatched_openings: Vec<DelimiterCheckpoint>,
    pub unexpected_closings: Vec<UnexpectedClosing>,
    pub mismatched_closings: Vec<MismatchedClosing>,
    pub final_depth: usize,
    pub max_depth: usize,
}

impl DelimiterReport {
    pub fn checkpoint_summaries(&self) -> Vec<String> {
        self.checkpoints
            .iter()
            .map(DelimiterCheckpoint::debug_summary)
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StackEntry {
    checkpoint_id: usize,
    kind: DelimiterKind,
    opening_position: InputPosition,
    depth: usize,
}

pub fn observe_delimiters(input: &str) -> DelimiterReport {
    let mut checkpoints = Vec::new();
    let mut matches = Vec::new();
    let mut unexpected_closings = Vec::new();
    let mut mismatched_closings = Vec::new();
    let mut stack: Vec<StackEntry> = Vec::new();
    let mut delimiter_index = 0;
    let mut max_depth = 0;

    for (char_index, (byte_offset, c)) in input.char_indices().enumerate() {
        let position = InputPosition {
            byte_offset,
            char_index,
            delimiter_index,
        };

        if let Some(kind) = DelimiterKind::from_open(c) {
            let depth = stack.len() + 1;
            let checkpoint_id = checkpoints.len();
            max_depth = max_depth.max(depth);
            checkpoints.push(DelimiterCheckpoint {
                id: checkpoint_id,
                kind,
                opening_position: position,
                depth,
                matched: false,
                closing_position: None,
                parser_state: None,
            });
            stack.push(StackEntry {
                checkpoint_id,
                kind,
                opening_position: position,
                depth,
            });
            delimiter_index += 1;
            continue;
        }

        if let Some(kind) = DelimiterKind::from_close(c) {
            match stack.pop() {
                Some(opening) if opening.kind == kind => {
                    checkpoints[opening.checkpoint_id].matched = true;
                    checkpoints[opening.checkpoint_id].closing_position = Some(position);
                    matches.push(DelimiterMatch {
                        checkpoint_id: opening.checkpoint_id,
                        kind,
                        opening_position: opening.opening_position,
                        closing_position: position,
                        depth: opening.depth,
                    });
                }
                Some(opening) => {
                    checkpoints[opening.checkpoint_id].closing_position = Some(position);
                    mismatched_closings.push(MismatchedClosing {
                        checkpoint_id: opening.checkpoint_id,
                        opening_kind: opening.kind,
                        opening_position: opening.opening_position,
                        closing_kind: kind,
                        closing_position: position,
                        expected_closing: opening.kind.close_char(),
                        actual_closing: kind.close_char(),
                        depth: opening.depth,
                    });
                }
                None => {
                    unexpected_closings.push(UnexpectedClosing {
                        kind,
                        position,
                        depth: 0,
                    });
                }
            }
            delimiter_index += 1;
        }
    }

    let unmatched_openings = stack
        .iter()
        .map(|entry| checkpoints[entry.checkpoint_id].clone())
        .collect::<Vec<_>>();

    DelimiterReport {
        input: input.to_string(),
        checkpoints,
        matches,
        unmatched_openings,
        unexpected_closings,
        mismatched_closings,
        final_depth: stack.len(),
        max_depth,
    }
}

#[cfg(test)]
mod tests {
    use super::{DelimiterKind, observe_delimiters};

    fn assert_balanced(input: &str, checkpoints: usize, max_depth: usize) {
        let report = observe_delimiters(input);
        assert_eq!(report.checkpoints.len(), checkpoints, "{input}");
        assert_eq!(report.matches.len(), checkpoints, "{input}");
        assert_eq!(report.unmatched_openings.len(), 0, "{input}");
        assert_eq!(report.unexpected_closings.len(), 0, "{input}");
        assert_eq!(report.mismatched_closings.len(), 0, "{input}");
        assert_eq!(report.final_depth, 0, "{input}");
        assert_eq!(report.max_depth, max_depth, "{input}");
        assert!(report.checkpoints.iter().all(|c| c.matched), "{input}");
    }

    #[test]
    fn valid_delimiters_are_matched() {
        assert_balanced("()", 1, 1);
        assert_balanced("{}", 1, 1);
        assert_balanced("[]", 1, 1);
        assert_balanced("({[]})", 3, 3);
        assert_balanced("(({}))", 3, 3);
    }

    #[test]
    fn unmatched_openings_remain_as_origin_checkpoints() {
        let cases = [
            ("(", vec![DelimiterKind::Parenthesis]),
            ("{", vec![DelimiterKind::Brace]),
            ("[", vec![DelimiterKind::Bracket]),
            (
                "({[",
                vec![
                    DelimiterKind::Parenthesis,
                    DelimiterKind::Brace,
                    DelimiterKind::Bracket,
                ],
            ),
        ];

        for (input, expected) in cases {
            let report = observe_delimiters(input);
            let kinds = report
                .unmatched_openings
                .iter()
                .map(|c| c.kind)
                .collect::<Vec<_>>();
            assert_eq!(kinds, expected, "{input}");
            assert_eq!(report.final_depth, expected.len(), "{input}");
            assert!(report.unexpected_closings.is_empty(), "{input}");
            assert!(report.mismatched_closings.is_empty(), "{input}");
            assert!(
                report.unmatched_openings.iter().all(|c| !c.matched),
                "{input}"
            );
        }
    }

    #[test]
    fn unexpected_closings_are_recorded_without_checkpoints() {
        let cases = [
            (")", DelimiterKind::Parenthesis),
            ("}", DelimiterKind::Brace),
            ("]", DelimiterKind::Bracket),
        ];

        for (input, expected) in cases {
            let report = observe_delimiters(input);
            assert!(report.checkpoints.is_empty(), "{input}");
            assert_eq!(report.unexpected_closings.len(), 1, "{input}");
            assert_eq!(report.unexpected_closings[0].kind, expected, "{input}");
            assert_eq!(
                report.unexpected_closings[0].position.byte_offset, 0,
                "{input}"
            );
            assert_eq!(report.final_depth, 0, "{input}");
        }
    }

    #[test]
    fn mismatched_closings_are_recorded_against_opening_checkpoint() {
        let cases = [("([)]", 2), ("{]", 1), ("[}", 1)];

        for (input, expected_mismatches) in cases {
            let report = observe_delimiters(input);
            assert_eq!(
                report.mismatched_closings.len(),
                expected_mismatches,
                "{input}"
            );
            assert_eq!(report.matches.len(), 0, "{input}");
            assert_eq!(report.unexpected_closings.len(), 0, "{input}");
            assert_eq!(report.final_depth, 0, "{input}");
            for mismatch in &report.mismatched_closings {
                let checkpoint = &report.checkpoints[mismatch.checkpoint_id];
                assert!(!checkpoint.matched, "{input}");
                assert_eq!(
                    checkpoint.closing_position,
                    Some(mismatch.closing_position),
                    "{input}"
                );
                assert_ne!(
                    mismatch.expected_closing, mismatch.actual_closing,
                    "{input}"
                );
            }
        }
    }

    #[test]
    fn nesting_and_multiple_pairs_are_recorded() {
        let deep = observe_delimiters("(((((((((())))))))))");
        assert_eq!(deep.checkpoints.len(), 10);
        assert_eq!(deep.matches.len(), 10);
        assert_eq!(deep.max_depth, 10);
        assert_eq!(deep.final_depth, 0);

        let independent = observe_delimiters("(){}[]");
        assert_eq!(independent.checkpoints.len(), 3);
        assert_eq!(independent.matches.len(), 3);
        assert_eq!(
            independent
                .checkpoints
                .iter()
                .map(|c| c.depth)
                .collect::<Vec<_>>(),
            vec![1, 1, 1]
        );

        let multiple_unmatched = observe_delimiters("((({[");
        assert_eq!(multiple_unmatched.unmatched_openings.len(), 5);
        assert_eq!(multiple_unmatched.final_depth, 5);
        assert_eq!(multiple_unmatched.max_depth, 5);
    }

    #[test]
    fn checkpoint_positions_are_deterministic() {
        let report = observe_delimiters("a({[]})");
        let checkpoints = report.checkpoints;
        assert_eq!(checkpoints[0].opening_position.byte_offset, 1);
        assert_eq!(checkpoints[0].opening_position.char_index, 1);
        assert_eq!(checkpoints[0].opening_position.delimiter_index, 0);
        assert_eq!(checkpoints[0].depth, 1);
        assert!(checkpoints[0].matched);
        assert_eq!(checkpoints[1].depth, 2);
        assert_eq!(checkpoints[2].depth, 3);
    }
}
