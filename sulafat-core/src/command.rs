//! Turning a host alias or an ad-hoc `user@host` target into the `ssh` argv to execute.
//!
//! This never talks to a process itself — the frontend owns spawning (via VTE), this module only
//! builds the argument vector. All SSH behavior (keys, `ssh-agent`, `known_hosts`, `ProxyJump`,
//! `~/.ssh/config`) is left entirely to the `ssh` binary.

/// What to connect to: a saved host alias (resolved through `~/.ssh/config` by `ssh` itself), or
/// an ad-hoc "quick connect" target that doesn't need a saved profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectTarget {
    Alias(String),
    Quick { user: Option<String>, host: String, port: Option<u16> },
}

/// Build the `ssh` argv for a target. Never includes a password or any other secret — those are
/// handled interactively by `ssh` itself inside the terminal.
pub fn build_ssh_command(target: &ConnectTarget) -> Vec<String> {
    match target {
        ConnectTarget::Alias(alias) => vec!["ssh".to_string(), alias.clone()],
        ConnectTarget::Quick { user, host, port } => {
            let mut argv = vec!["ssh".to_string()];
            if let Some(port) = port {
                argv.push("-p".to_string());
                argv.push(port.to_string());
            }
            let destination = match user {
                Some(user) => format!("{user}@{host}"),
                None => host.clone(),
            };
            argv.push(destination);
            argv
        }
    }
}

/// Parse a quick-connect entry of the form `[usuário@]host[:porta]`.
pub fn parse_quick_target(input: &str) -> Option<ConnectTarget> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    let (user, rest) = match input.split_once('@') {
        Some((user, rest)) if !user.is_empty() => (Some(user.to_string()), rest),
        _ => (None, input),
    };
    if rest.is_empty() {
        return None;
    }
    let (host, port) = match rest.rsplit_once(':') {
        Some((host, port_str)) if !host.is_empty() && !port_str.is_empty() => {
            match port_str.parse::<u16>() {
                Ok(port) => (host.to_string(), Some(port)),
                Err(_) => (rest.to_string(), None),
            }
        }
        _ => (rest.to_string(), None),
    };
    if host.is_empty() {
        return None;
    }
    Some(ConnectTarget::Quick { user, host, port })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alias_command_is_just_ssh_and_the_alias() {
        assert_eq!(build_ssh_command(&ConnectTarget::Alias("prod".into())), vec!["ssh", "prod"]);
    }

    #[test]
    fn quick_command_includes_user_and_port_when_present() {
        let target = ConnectTarget::Quick { user: Some("admin".into()), host: "10.0.0.1".into(), port: Some(2222) };
        assert_eq!(build_ssh_command(&target), vec!["ssh", "-p", "2222", "admin@10.0.0.1"]);
    }

    #[test]
    fn quick_command_without_user_or_port() {
        let target = ConnectTarget::Quick { user: None, host: "10.0.0.1".into(), port: None };
        assert_eq!(build_ssh_command(&target), vec!["ssh", "10.0.0.1"]);
    }

    #[test]
    fn parses_user_host_port() {
        let target = parse_quick_target("admin@10.0.0.1:2222").unwrap();
        assert_eq!(target, ConnectTarget::Quick { user: Some("admin".into()), host: "10.0.0.1".into(), port: Some(2222) });
    }

    #[test]
    fn parses_bare_host() {
        let target = parse_quick_target("10.0.0.1").unwrap();
        assert_eq!(target, ConnectTarget::Quick { user: None, host: "10.0.0.1".into(), port: None });
    }

    #[test]
    fn non_numeric_trailing_segment_is_kept_as_part_of_the_host() {
        // The text after the last ':' isn't a valid port, so it's treated as part of the host
        // rather than silently dropped.
        let target = parse_quick_target("user@host:abc").unwrap();
        assert_eq!(target, ConnectTarget::Quick { user: Some("user".into()), host: "host:abc".into(), port: None });
    }

    #[test]
    fn empty_input_is_rejected() {
        assert_eq!(parse_quick_target("   "), None);
        assert_eq!(parse_quick_target("user@"), None);
    }
}
