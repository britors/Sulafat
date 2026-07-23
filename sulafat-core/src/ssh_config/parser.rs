//! Line-based parsing of `~/.ssh/config` that preserves the original bytes exactly.
//!
//! Every line is kept verbatim, including its own line terminator (or the lack of one, for a
//! final line with no trailing newline). A [`Segment`](super::Segment) is nothing more than an
//! ordered slice of those lines, so re-rendering the parsed structure with no edits reproduces
//! the original file byte for byte — there is no separate "pretty-printer" that could drift from
//! the input's formatting.

use super::{BlockLine, KnownDirective, ManagedBlock, RawLine, Segment};

/// What a top-level (unindented in spirit, though `ssh_config` ignores indentation) line means
/// for block boundaries.
pub(super) enum LineKind {
    HostHeader(Vec<String>),
    MatchHeader,
    Other,
}

/// Split `text` into lines that each retain their own trailing `\n` (or nothing, for a final
/// line without one). Concatenating every returned line reproduces `text` exactly.
fn split_raw_lines(text: &str) -> Vec<RawLine> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            lines.push(RawLine(text[start..=i].to_string()));
            start = i + 1;
        }
    }
    if start < text.len() {
        lines.push(RawLine(text[start..].to_string()));
    }
    lines
}

/// The line's content with its trailing `\r\n`/`\n` terminator (if any) removed.
pub(super) fn strip_terminator(line: &str) -> &str {
    match line.strip_suffix('\n') {
        Some(rest) => rest.strip_suffix('\r').unwrap_or(rest),
        None => line,
    }
}

fn is_wildcard_pattern(pattern: &str) -> bool {
    pattern.contains('*') || pattern.contains('?') || pattern.starts_with('!')
}

/// A single pattern, and only a single pattern, with no glob/negation characters — the only
/// shape of `Host` block this app will ever rewrite.
pub(super) fn is_single_plain_pattern(patterns: &[String]) -> bool {
    matches!(patterns, [only] if !is_wildcard_pattern(only))
}

pub(super) fn classify_top(content: &str) -> LineKind {
    let trimmed = content.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return LineKind::Other;
    }
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let keyword = parts.next().unwrap_or("");
    if keyword.eq_ignore_ascii_case("host") {
        let rest = parts.next().unwrap_or("").trim();
        let patterns = rest.split_whitespace().map(str::to_string).collect();
        LineKind::HostHeader(patterns)
    } else if keyword.eq_ignore_ascii_case("match") {
        LineKind::MatchHeader
    } else {
        LineKind::Other
    }
}

/// Classify every line of a single-pattern `Host` block's body: the first occurrence of each
/// known directive becomes [`BlockLine::Known`], everything else (comments, blank lines, unknown
/// directives, and repeated occurrences of an already-seen known directive) becomes
/// [`BlockLine::Other`] and is preserved verbatim in original order.
fn classify_body(body: Vec<RawLine>) -> Vec<BlockLine> {
    let mut seen = [false; KnownDirective::ALL.len()];
    body.into_iter()
        .map(|raw| {
            let content = strip_terminator(&raw.0);
            let trimmed = content.trim_start();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                return BlockLine::Other(raw);
            }
            let keyword = trimmed.split(char::is_whitespace).next().unwrap_or("");
            if let Some(directive) = KnownDirective::from_keyword(keyword) {
                let idx = directive.index();
                if !seen[idx] {
                    seen[idx] = true;
                    return BlockLine::Known { directive, line: raw };
                }
            }
            BlockLine::Other(raw)
        })
        .collect()
}

/// Parse a whole `~/.ssh/config`-style document into segments.
pub(super) fn parse(text: &str) -> Vec<Segment> {
    let raw_lines = split_raw_lines(text);
    let mut segments = Vec::new();
    let mut current_raw: Vec<RawLine> = Vec::new();
    let mut i = 0;

    while i < raw_lines.len() {
        let content = strip_terminator(&raw_lines[i].0).to_string();
        match classify_top(&content) {
            LineKind::HostHeader(patterns) => {
                if !current_raw.is_empty() {
                    segments.push(Segment::Raw(std::mem::take(&mut current_raw)));
                }
                let header = raw_lines[i].clone();
                i += 1;
                let mut body = Vec::new();
                while i < raw_lines.len() {
                    let c = strip_terminator(&raw_lines[i].0);
                    if matches!(classify_top(c), LineKind::HostHeader(_) | LineKind::MatchHeader) {
                        break;
                    }
                    body.push(raw_lines[i].clone());
                    i += 1;
                }
                if is_single_plain_pattern(&patterns) {
                    // A trailing run of purely-blank lines reads as separator whitespace before
                    // whatever comes next, not as part of this block — otherwise removing the
                    // block later would also swallow that spacing. Comments right before the
                    // next block are left alone; there's no reliable way to tell whether they
                    // belong to this entry or the next one, and they're an edge case either way.
                    let mut trailing_blanks = Vec::new();
                    while matches!(body.last(), Some(l) if strip_terminator(&l.0).trim().is_empty()) {
                        trailing_blanks.push(body.pop().expect("just checked with body.last()"));
                    }
                    trailing_blanks.reverse();

                    segments.push(Segment::Managed(ManagedBlock {
                        alias: patterns[0].clone(),
                        header,
                        lines: classify_body(body),
                    }));
                    current_raw.extend(trailing_blanks);
                } else {
                    let mut block_lines = vec![header];
                    block_lines.extend(body);
                    segments.push(Segment::Raw(block_lines));
                }
            }
            LineKind::MatchHeader => {
                if !current_raw.is_empty() {
                    segments.push(Segment::Raw(std::mem::take(&mut current_raw)));
                }
                let mut block_lines = vec![raw_lines[i].clone()];
                i += 1;
                while i < raw_lines.len() {
                    let c = strip_terminator(&raw_lines[i].0);
                    if matches!(classify_top(c), LineKind::HostHeader(_) | LineKind::MatchHeader) {
                        break;
                    }
                    block_lines.push(raw_lines[i].clone());
                    i += 1;
                }
                segments.push(Segment::Raw(block_lines));
            }
            LineKind::Other => {
                current_raw.push(raw_lines[i].clone());
                i += 1;
            }
        }
    }
    if !current_raw.is_empty() {
        segments.push(Segment::Raw(current_raw));
    }
    segments
}

/// Re-render segments back into a document. With no edits, this reproduces the original text
/// exactly, since every segment is just the original lines it was built from.
pub(super) fn render(segments: &[Segment]) -> String {
    let mut out = String::new();
    for seg in segments {
        match seg {
            Segment::Raw(lines) => {
                for line in lines {
                    out.push_str(&line.0);
                }
            }
            Segment::Managed(block) => {
                out.push_str(&block.header.0);
                for line in &block.lines {
                    match line {
                        BlockLine::Known { line, .. } => out.push_str(&line.0),
                        BlockLine::Other(line) => out.push_str(&line.0),
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(text: &str) {
        let segments = parse(text);
        assert_eq!(render(&segments), text, "round-trip mismatch for:\n{text:?}");
    }

    #[test]
    fn empty_file_roundtrips() {
        roundtrip("");
    }

    #[test]
    fn comments_and_blank_lines_roundtrip() {
        roundtrip("# top comment\n\n# another\nHost prod\n    HostName 10.0.0.1\n\n# trailing\n");
    }

    #[test]
    fn unknown_directives_are_preserved_verbatim() {
        roundtrip(
            "Host prod\n    HostName 10.0.0.1\n    ServerAliveInterval 30\n    # note\n    Compression yes\n",
        );
    }

    #[test]
    fn wildcard_and_multi_pattern_hosts_stay_raw() {
        roundtrip("Host *\n    ServerAliveInterval 60\n\nHost web1 web2\n    User deploy\n");
    }

    #[test]
    fn file_without_trailing_newline_roundtrips() {
        roundtrip("Host prod\n    HostName 10.0.0.1");
    }

    #[test]
    fn include_directive_roundtrips() {
        roundtrip("Include conf.d/*.conf\n\nHost prod\n    HostName 10.0.0.1\n");
    }

    #[test]
    fn match_block_stays_raw() {
        roundtrip("Match host prod\n    ForwardAgent yes\n\nHost staging\n    User dev\n");
    }

    #[test]
    fn managed_block_classifies_known_and_extra_lines() {
        let segments = parse("Host prod\n    HostName 10.0.0.1\n    User admin\n    Compression yes\n");
        let Segment::Managed(block) = &segments[0] else {
            panic!("expected a managed block")
        };
        assert_eq!(block.alias, "prod");
        assert!(matches!(
            &block.lines[0],
            BlockLine::Known { directive: KnownDirective::HostName, .. }
        ));
        assert!(matches!(
            &block.lines[1],
            BlockLine::Known { directive: KnownDirective::User, .. }
        ));
        assert!(matches!(&block.lines[2], BlockLine::Other(_)));
    }

    #[test]
    fn repeated_known_directive_only_first_occurrence_is_known() {
        let segments = parse("Host prod\n    Port 22\n    Port 2222\n");
        let Segment::Managed(block) = &segments[0] else {
            panic!("expected a managed block")
        };
        assert!(matches!(
            &block.lines[0],
            BlockLine::Known { directive: KnownDirective::Port, .. }
        ));
        assert!(matches!(&block.lines[1], BlockLine::Other(_)));
    }
}
