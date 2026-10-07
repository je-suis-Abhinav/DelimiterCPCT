use crate::{
    BaselineReport,
    delimiter::{
        DelimiterCheckpoint, DelimiterKind, DelimiterReport, InputPosition, MismatchedClosing,
        UnexpectedClosing,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParserErrorKind {
    Lexing,
    Parsing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParserErrorPosition {
    pub kind: ParserErrorKind,
    pub byte_offset: usize,
    pub char_index: usize,
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalizationReason {
    UnmatchedOpening,
    UnexpectedClosing,
    MismatchedClosing,
    MatchedStructureBeforeError,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalizedDelimiterCandidate {
    pub reason: LocalizationReason,
    pub delimiter: DelimiterKind,
    pub checkpoint: Option<DelimiterCheckpoint>,
    pub event_position: InputPosition,
    pub distance_from_parser_error: Option<usize>,
    pub depth: usize,
    pub matched: Option<bool>,
    pub relationship: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuralLocalization {
    pub parser_error_position: Option<ParserErrorPosition>,
    pub final_delimiter_depth: usize,
    pub max_delimiter_depth: usize,
    pub candidates: Vec<LocalizedDelimiterCandidate>,
}

impl StructuralLocalization {
    pub fn debug_summaries(&self) -> Vec<String> {
        self.candidates
            .iter()
            .map(|candidate| {
                let error = self
                    .parser_error_position
                    .map(|p| format!("byte={}, char={}", p.byte_offset, p.char_index))
                    .unwrap_or_else(|| "none".to_string());
                let distance = candidate
                    .distance_from_parser_error
                    .map(|d| d.to_string())
                    .unwrap_or_else(|| "n/a".to_string());
                format!(
                    "reason={:?}, delimiter={}, event_byte={}, event_char={}, depth={}, matched={:?}, parser_error={}, distance={}, relationship={}",
                    candidate.reason,
                    candidate.delimiter.open_char(),
                    candidate.event_position.byte_offset,
                    candidate.event_position.char_index,
                    candidate.depth,
                    candidate.matched,
                    error,
                    distance,
                    candidate.relationship
                )
            })
            .collect()
    }
}

pub fn localize_structure(
    baseline: &BaselineReport,
    delimiters: &DelimiterReport,
) -> StructuralLocalization {
    let parser_error_position = baseline.error_positions.first().copied();
    let mut candidates = Vec::new();

    for checkpoint in ranked_unmatched_openings(delimiters) {
        candidates.push(candidate_from_unmatched(checkpoint, parser_error_position));
    }

    for event in ranked_unexpected_closings(delimiters, parser_error_position) {
        candidates.push(candidate_from_unexpected(event, parser_error_position));
    }

    for mismatch in ranked_mismatches(delimiters, parser_error_position) {
        candidates.push(candidate_from_mismatch(
            delimiters,
            mismatch,
            parser_error_position,
        ));
    }

    if candidates.is_empty() && parser_error_position.is_some() {
        for checkpoint in ranked_matched_before_error(delimiters, parser_error_position.unwrap()) {
            candidates.push(candidate_from_matched_context(
                checkpoint,
                parser_error_position,
            ));
        }
    }

    StructuralLocalization {
        parser_error_position,
        final_delimiter_depth: delimiters.final_depth,
        max_delimiter_depth: delimiters.max_depth,
        candidates,
    }
}

fn ranked_unmatched_openings(delimiters: &DelimiterReport) -> Vec<&DelimiterCheckpoint> {
    let mut candidates = delimiters.unmatched_openings.iter().collect::<Vec<_>>();
    candidates.sort_by(|a, b| {
        b.depth
            .cmp(&a.depth)
            .then_with(|| {
                b.opening_position
                    .byte_offset
                    .cmp(&a.opening_position.byte_offset)
            })
            .then_with(|| a.id.cmp(&b.id))
    });
    candidates
}

fn ranked_unexpected_closings(
    delimiters: &DelimiterReport,
    parser_error_position: Option<ParserErrorPosition>,
) -> Vec<&UnexpectedClosing> {
    let mut candidates = delimiters.unexpected_closings.iter().collect::<Vec<_>>();
    candidates.sort_by(|a, b| {
        distance(parser_error_position, a.position)
            .cmp(&distance(parser_error_position, b.position))
            .then_with(|| a.position.byte_offset.cmp(&b.position.byte_offset))
    });
    candidates
}

fn ranked_mismatches(
    delimiters: &DelimiterReport,
    parser_error_position: Option<ParserErrorPosition>,
) -> Vec<&MismatchedClosing> {
    let mut candidates = delimiters.mismatched_closings.iter().collect::<Vec<_>>();
    candidates.sort_by(|a, b| {
        distance(parser_error_position, a.closing_position)
            .cmp(&distance(parser_error_position, b.closing_position))
            .then_with(|| b.depth.cmp(&a.depth))
            .then_with(|| {
                a.opening_position
                    .byte_offset
                    .cmp(&b.opening_position.byte_offset)
            })
    });
    candidates
}

fn ranked_matched_before_error(
    delimiters: &DelimiterReport,
    parser_error_position: ParserErrorPosition,
) -> Vec<&DelimiterCheckpoint> {
    let mut candidates = delimiters
        .checkpoints
        .iter()
        .filter(|checkpoint| {
            checkpoint.matched
                && checkpoint
                    .closing_position
                    .map(|p| p.byte_offset <= parser_error_position.byte_offset)
                    .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|a, b| {
        b.closing_position
            .unwrap()
            .byte_offset
            .cmp(&a.closing_position.unwrap().byte_offset)
            .then_with(|| b.depth.cmp(&a.depth))
    });
    candidates
}

fn candidate_from_unmatched(
    checkpoint: &DelimiterCheckpoint,
    parser_error_position: Option<ParserErrorPosition>,
) -> LocalizedDelimiterCandidate {
    LocalizedDelimiterCandidate {
        reason: LocalizationReason::UnmatchedOpening,
        delimiter: checkpoint.kind,
        checkpoint: Some(checkpoint.clone()),
        event_position: checkpoint.opening_position,
        distance_from_parser_error: distance(parser_error_position, checkpoint.opening_position),
        depth: checkpoint.depth,
        matched: Some(false),
        relationship: "opening delimiter remains unmatched at parser error observation time"
            .to_string(),
    }
}

fn candidate_from_unexpected(
    event: &UnexpectedClosing,
    parser_error_position: Option<ParserErrorPosition>,
) -> LocalizedDelimiterCandidate {
    LocalizedDelimiterCandidate {
        reason: LocalizationReason::UnexpectedClosing,
        delimiter: event.kind,
        checkpoint: None,
        event_position: event.position,
        distance_from_parser_error: distance(parser_error_position, event.position),
        depth: event.depth,
        matched: None,
        relationship: "closing delimiter appeared without an active opening delimiter".to_string(),
    }
}

fn candidate_from_mismatch(
    delimiters: &DelimiterReport,
    mismatch: &MismatchedClosing,
    parser_error_position: Option<ParserErrorPosition>,
) -> LocalizedDelimiterCandidate {
    LocalizedDelimiterCandidate {
        reason: LocalizationReason::MismatchedClosing,
        delimiter: mismatch.opening_kind,
        checkpoint: Some(delimiters.checkpoints[mismatch.checkpoint_id].clone()),
        event_position: mismatch.closing_position,
        distance_from_parser_error: distance(parser_error_position, mismatch.closing_position),
        depth: mismatch.depth,
        matched: Some(false),
        relationship: format!(
            "opening delimiter expected {} but observed {}",
            mismatch.expected_closing, mismatch.actual_closing
        ),
    }
}

fn candidate_from_matched_context(
    checkpoint: &DelimiterCheckpoint,
    parser_error_position: Option<ParserErrorPosition>,
) -> LocalizedDelimiterCandidate {
    LocalizedDelimiterCandidate {
        reason: LocalizationReason::MatchedStructureBeforeError,
        delimiter: checkpoint.kind,
        checkpoint: Some(checkpoint.clone()),
        event_position: checkpoint
            .closing_position
            .unwrap_or(checkpoint.opening_position),
        distance_from_parser_error: distance(
            parser_error_position,
            checkpoint
                .closing_position
                .unwrap_or(checkpoint.opening_position),
        ),
        depth: checkpoint.depth,
        matched: Some(true),
        relationship: "parser error occurs after a matched delimiter structure".to_string(),
    }
}

fn distance(
    parser_error_position: Option<ParserErrorPosition>,
    event_position: InputPosition,
) -> Option<usize> {
    parser_error_position.map(|p| p.byte_offset.abs_diff(event_position.byte_offset))
}

#[cfg(test)]
mod tests {
    use crate::{
        delimiter::DelimiterKind,
        localization::{LocalizationReason, localize_structure},
        parse_baseline_with_delimiter_observation,
    };

    #[test]
    fn localizes_single_unmatched_opening() {
        let observed = parse_baseline_with_delimiter_observation("(1+2");
        let localization = localize_structure(&observed.baseline, &observed.delimiters);
        assert_eq!(localization.candidates.len(), 1);
        let candidate = &localization.candidates[0];
        assert_eq!(candidate.reason, LocalizationReason::UnmatchedOpening);
        assert_eq!(candidate.delimiter, DelimiterKind::Parenthesis);
        assert_eq!(candidate.depth, 1);
        assert_eq!(candidate.event_position.byte_offset, 0);
        assert_eq!(candidate.matched, Some(false));
    }

    #[test]
    fn localizes_outer_unmatched_opening_when_inner_is_matched() {
        let observed = parse_baseline_with_delimiter_observation("((1+2)");
        let localization = localize_structure(&observed.baseline, &observed.delimiters);
        assert_eq!(localization.candidates.len(), 1);
        let candidate = &localization.candidates[0];
        assert_eq!(candidate.reason, LocalizationReason::UnmatchedOpening);
        assert_eq!(candidate.depth, 1);
        assert_eq!(candidate.event_position.byte_offset, 0);
        assert_eq!(observed.delimiters.checkpoints[1].matched, true);
        assert_eq!(observed.delimiters.checkpoints[0].matched, false);
    }

    #[test]
    fn localizes_unexpected_closing_without_opening_candidate() {
        let observed = parse_baseline_with_delimiter_observation("1+2)");
        let localization = localize_structure(&observed.baseline, &observed.delimiters);
        assert_eq!(observed.delimiters.unmatched_openings.len(), 0);
        assert_eq!(localization.candidates.len(), 1);
        let candidate = &localization.candidates[0];
        assert_eq!(candidate.reason, LocalizationReason::UnexpectedClosing);
        assert_eq!(candidate.delimiter, DelimiterKind::Parenthesis);
        assert!(candidate.checkpoint.is_none());
        assert_eq!(candidate.event_position.byte_offset, 3);
    }

    #[test]
    fn localizes_mismatched_delimiter_and_preserves_opening_checkpoint() {
        let observed = parse_baseline_with_delimiter_observation("([1+2}");
        let localization = localize_structure(&observed.baseline, &observed.delimiters);
        assert_eq!(observed.delimiters.mismatched_closings.len(), 1);
        assert_eq!(localization.candidates.len(), 2);
        assert_eq!(
            localization.candidates[0].reason,
            LocalizationReason::UnmatchedOpening
        );
        assert_eq!(
            localization.candidates[1].reason,
            LocalizationReason::MismatchedClosing
        );
        assert_eq!(localization.candidates[1].depth, 2);
        assert!(localization.candidates[1].checkpoint.is_some());
    }

    #[test]
    fn preserves_multiple_unmatched_candidates_deterministically() {
        let observed = parse_baseline_with_delimiter_observation("({[1+2");
        let localization = localize_structure(&observed.baseline, &observed.delimiters);
        assert_eq!(localization.candidates.len(), 3);
        assert_eq!(
            localization
                .candidates
                .iter()
                .map(|c| c.depth)
                .collect::<Vec<_>>(),
            vec![3, 2, 1]
        );
        assert_eq!(
            localization
                .candidates
                .iter()
                .map(|c| c.delimiter)
                .collect::<Vec<_>>(),
            vec![
                DelimiterKind::Bracket,
                DelimiterKind::Brace,
                DelimiterKind::Parenthesis
            ]
        );
    }

    #[test]
    fn balanced_valid_input_has_no_false_localization() {
        let observed = parse_baseline_with_delimiter_observation("(1+2)*3");
        let localization = localize_structure(&observed.baseline, &observed.delimiters);
        assert!(localization.parser_error_position.is_none());
        assert!(localization.candidates.is_empty());
    }

    #[test]
    fn parser_error_after_matched_structure_is_identified_as_context_only() {
        let observed = parse_baseline_with_delimiter_observation("(1+2)+*");
        let localization = localize_structure(&observed.baseline, &observed.delimiters);
        assert_eq!(
            localization.candidates[0].reason,
            LocalizationReason::MatchedStructureBeforeError
        );
        assert_eq!(localization.candidates[0].matched, Some(true));
    }
}
