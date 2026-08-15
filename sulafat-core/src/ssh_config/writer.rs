//! Surgical, in-memory editing of parsed [`Segment`]s.
//!
//! Every function here mutates only the lines that actually changed: unmapped directives,
//! comments and blank lines inside an edited block are left untouched, and every other block in
//! the file is never even visited. File I/O (atomic write, backup, permissions) lives in
//! [`super::SshConfig::save`], not here — this module only ever touches the in-memory model.

use super::{BlockLine, KnownDirective, ManagedBlock, RawLine, Segment, SshHost};

fn find_managed_index(segments: &[Segment], alias: &str) -> Option<usize> {
    segments
        .iter()
        .position(|s| matches!(s, Segment::Managed(b) if b.alias == alias))
}

fn find_known_index(block: &ManagedBlock, directive: KnownDirective) -> Option<usize> {
    block
        .lines
        .iter()
        .position(|l| matches!(l, BlockLine::Known { directive: d, .. } if *d == directive))
}

fn last_known_index(block: &ManagedBlock) -> Option<usize> {
    block
        .lines
        .iter()
        .rposition(|l| matches!(l, BlockLine::Known { .. }))
}

/// Split a raw line into `(leading_whitespace_and_keyword, terminator)`, so a replacement value
/// can be spliced in while keeping the original indentation, keyword casing and line ending.
fn split_keyword_and_terminator(line: &str) -> (&str, &str) {
    let content = super::parser::strip_terminator(line);
    let terminator = &line[content.len()..];
    let keyword_start = content.len() - content.trim_start().len();
    let after_ws = &content[keyword_start..];
    let keyword_len = after_ws.find(char::is_whitespace).unwrap_or(after_ws.len());
    (&content[..keyword_start + keyword_len], terminator)
}

fn replace_line_value(line: &mut RawLine, new_value: &str) {
    let (keyword_part, terminator) = split_keyword_and_terminator(&line.0);
    line.0 = format!("{keyword_part} {new_value}{terminator}");
}

fn set_header_alias(header: &mut RawLine, new_alias: &str) {
    let (keyword_part, terminator) = split_keyword_and_terminator(&header.0);
    header.0 = format!("{keyword_part} {new_alias}{terminator}");
}

fn build_known_line(directive: KnownDirective, value: &str) -> RawLine {
    RawLine(format!("    {} {value}\n", directive.keyword()))
}

fn apply_known_field(block: &mut ManagedBlock, directive: KnownDirective, new_value: Option<&str>) {
    match (find_known_index(block, directive), new_value) {
        (Some(idx), None) => {
            block.lines.remove(idx);
        }
        (Some(idx), Some(v)) => {
            if let BlockLine::Known { line, .. } = &mut block.lines[idx] {
                replace_line_value(line, v);
            }
        }
        (None, Some(v)) => {
            let insert_at = last_known_index(block).map(|i| i + 1).unwrap_or(0);
            block.lines.insert(
                insert_at,
                BlockLine::Known {
                    directive,
                    line: build_known_line(directive, v),
                },
            );
        }
        (None, None) => {}
    }
}

/// Replace the block's free-form "advanced options" text (everything that isn't one of the known
/// fields) wholesale, keeping it grouped right after the known-directive lines.
fn replace_extra_lines(block: &mut ManagedBlock, extra: &str) {
    block.lines.retain(|l| matches!(l, BlockLine::Known { .. }));
    if !extra.is_empty() {
        for line in extra.split('\n') {
            block
                .lines
                .push(BlockLine::Other(RawLine(format!("{line}\n"))));
        }
    }
}

fn port_value(host: &SshHost) -> Option<String> {
    host.port.map(|p| p.to_string())
}

fn rewrite_block(block: &mut ManagedBlock, host: &SshHost) {
    if host.alias != block.alias {
        set_header_alias(&mut block.header, &host.alias);
        block.alias = host.alias.clone();
    }
    apply_known_field(block, KnownDirective::HostName, host.host_name.as_deref());
    apply_known_field(block, KnownDirective::User, host.user.as_deref());
    apply_known_field(block, KnownDirective::Port, port_value(host).as_deref());
    apply_known_field(
        block,
        KnownDirective::IdentityFile,
        host.identity_file.as_deref(),
    );
    apply_known_field(block, KnownDirective::ProxyJump, host.proxy_jump.as_deref());
    replace_extra_lines(block, &host.extra);
}

fn build_new_block(host: &SshHost) -> ManagedBlock {
    let mut block = ManagedBlock {
        alias: host.alias.clone(),
        header: RawLine(format!("Host {}\n", host.alias)),
        lines: Vec::new(),
    };
    for (directive, value) in [
        (KnownDirective::HostName, host.host_name.clone()),
        (KnownDirective::User, host.user.clone()),
        (KnownDirective::Port, port_value(host)),
        (KnownDirective::IdentityFile, host.identity_file.clone()),
        (KnownDirective::ProxyJump, host.proxy_jump.clone()),
    ] {
        if let Some(v) = value {
            block.lines.push(BlockLine::Known {
                directive,
                line: build_known_line(directive, &v),
            });
        }
    }
    if !host.extra.is_empty() {
        for line in host.extra.split('\n') {
            block
                .lines
                .push(BlockLine::Other(RawLine(format!("{line}\n"))));
        }
    }
    block
}

fn last_raw_line_mut(segments: &mut [Segment]) -> Option<&mut RawLine> {
    match segments.last_mut()? {
        Segment::Raw(lines) => lines.last_mut(),
        Segment::Managed(block) => match block.lines.last_mut() {
            Some(BlockLine::Known { line, .. }) => Some(line),
            Some(BlockLine::Other(line)) => Some(line),
            None => Some(&mut block.header),
        },
    }
}

fn push_raw_line(segments: &mut Vec<Segment>, text: &str) {
    match segments.last_mut() {
        Some(Segment::Raw(lines)) => lines.push(RawLine(text.to_string())),
        _ => segments.push(Segment::Raw(vec![RawLine(text.to_string())])),
    }
}

/// Make sure a brand-new block can be appended cleanly: terminate the file's last line if it was
/// missing a trailing newline, then add one blank separator line (skipped for an empty file).
fn ensure_appendable(segments: &mut Vec<Segment>) {
    if segments.is_empty() {
        return;
    }
    if let Some(last) = last_raw_line_mut(segments) {
        if !last.0.ends_with('\n') {
            last.0.push('\n');
        }
    }
    push_raw_line(segments, "\n");
}

pub(super) fn upsert(segments: &mut Vec<Segment>, host: &SshHost) {
    match find_managed_index(segments, &host.alias) {
        Some(idx) => {
            if let Segment::Managed(block) = &mut segments[idx] {
                rewrite_block(block, host);
            }
        }
        None => {
            ensure_appendable(segments);
            segments.push(Segment::Managed(build_new_block(host)));
        }
    }
}

/// Upsert under a possibly-renamed alias: `previous_alias` locates the block to edit even when
/// `host.alias` differs from it (renaming), falling back to appending a new block otherwise.
pub(super) fn upsert_renaming(
    segments: &mut Vec<Segment>,
    previous_alias: Option<&str>,
    host: &SshHost,
) {
    let idx = previous_alias.and_then(|a| find_managed_index(segments, a));
    match idx {
        Some(idx) => {
            if let Segment::Managed(block) = &mut segments[idx] {
                rewrite_block(block, host);
            }
        }
        None => upsert(segments, host),
    }
}

pub(super) fn remove(segments: &mut Vec<Segment>, alias: &str) -> bool {
    match find_managed_index(segments, alias) {
        Some(idx) => {
            segments.remove(idx);
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::super::parser::{parse, render};
    use super::*;

    fn parsed(text: &str) -> Vec<Segment> {
        parse(text)
    }

    #[test]
    fn editing_a_field_preserves_unrelated_lines() {
        let mut segments = parsed("# comment\nHost prod\n    HostName 10.0.0.1\n    Compression yes\n\nHost other\n    User x\n");
        // A real caller round-trips `extra` from the host it just loaded (here, "Compression
        // yes", the block's one unmapped directive) unless the user edited "Opções avançadas".
        let host = SshHost {
            alias: "prod".into(),
            host_name: Some("10.0.0.2".into()),
            extra: "    Compression yes".into(),
            ..Default::default()
        };
        upsert(&mut segments, &host);
        let out = render(&segments);
        assert_eq!(out, "# comment\nHost prod\n    HostName 10.0.0.2\n    Compression yes\n\nHost other\n    User x\n");
    }

    #[test]
    fn clearing_a_field_removes_its_line() {
        let mut segments = parsed("Host prod\n    HostName 10.0.0.1\n    User admin\n");
        let host = SshHost {
            alias: "prod".into(),
            host_name: Some("10.0.0.1".into()),
            user: None,
            ..Default::default()
        };
        upsert(&mut segments, &host);
        assert_eq!(render(&segments), "Host prod\n    HostName 10.0.0.1\n");
    }

    #[test]
    fn adding_a_new_field_inserts_after_known_lines() {
        let mut segments = parsed("Host prod\n    HostName 10.0.0.1\n    # note\n");
        // Round-trips the existing "# note" extra line, as a real caller would after loading it.
        let host = SshHost {
            alias: "prod".into(),
            host_name: Some("10.0.0.1".into()),
            user: Some("admin".into()),
            extra: "    # note".into(),
            ..Default::default()
        };
        upsert(&mut segments, &host);
        assert_eq!(
            render(&segments),
            "Host prod\n    HostName 10.0.0.1\n    User admin\n    # note\n"
        );
    }

    #[test]
    fn advanced_options_text_replaces_extra_lines_only() {
        let mut segments =
            parsed("Host prod\n    HostName 10.0.0.1\n    Compression yes\n    # old note\n");
        let host = SshHost {
            alias: "prod".into(),
            host_name: Some("10.0.0.1".into()),
            extra: "ServerAliveInterval 30\n# new note".into(),
            ..Default::default()
        };
        upsert(&mut segments, &host);
        assert_eq!(
            render(&segments),
            "Host prod\n    HostName 10.0.0.1\nServerAliveInterval 30\n# new note\n"
        );
    }

    #[test]
    fn new_host_is_appended_with_separator() {
        let mut segments = parsed("Host prod\n    HostName 10.0.0.1\n");
        let host = SshHost {
            alias: "staging".into(),
            host_name: Some("10.0.0.2".into()),
            ..Default::default()
        };
        upsert(&mut segments, &host);
        assert_eq!(
            render(&segments),
            "Host prod\n    HostName 10.0.0.1\n\nHost staging\n    HostName 10.0.0.2\n"
        );
    }

    #[test]
    fn new_host_on_file_missing_trailing_newline_still_terminates_previous_block() {
        let mut segments = parsed("Host prod\n    HostName 10.0.0.1");
        let host = SshHost {
            alias: "staging".into(),
            host_name: Some("10.0.0.2".into()),
            ..Default::default()
        };
        upsert(&mut segments, &host);
        assert_eq!(
            render(&segments),
            "Host prod\n    HostName 10.0.0.1\n\nHost staging\n    HostName 10.0.0.2\n"
        );
    }

    #[test]
    fn new_host_on_empty_file_has_no_leading_separator() {
        let mut segments = parsed("");
        let host = SshHost {
            alias: "staging".into(),
            host_name: Some("10.0.0.2".into()),
            ..Default::default()
        };
        upsert(&mut segments, &host);
        assert_eq!(render(&segments), "Host staging\n    HostName 10.0.0.2\n");
    }

    #[test]
    fn removing_a_host_deletes_its_whole_block() {
        let mut segments =
            parsed("Host prod\n    HostName 10.0.0.1\n\nHost staging\n    User dev\n");
        assert!(remove(&mut segments, "prod"));
        assert_eq!(render(&segments), "\nHost staging\n    User dev\n");
    }

    #[test]
    fn removing_an_unknown_alias_is_a_no_op_and_reports_false() {
        let mut segments = parsed("Host prod\n    HostName 10.0.0.1\n");
        assert!(!remove(&mut segments, "ghost"));
        assert_eq!(render(&segments), "Host prod\n    HostName 10.0.0.1\n");
    }

    #[test]
    fn renaming_alias_updates_header_and_keeps_matching_by_previous_alias() {
        let mut segments = parsed("Host prod\n    HostName 10.0.0.1\n");
        let host = SshHost {
            alias: "prod-db".into(),
            host_name: Some("10.0.0.1".into()),
            ..Default::default()
        };
        upsert_renaming(&mut segments, Some("prod"), &host);
        assert_eq!(render(&segments), "Host prod-db\n    HostName 10.0.0.1\n");
    }
}
