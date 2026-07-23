//! Core engine for Sulafat: fidelity-preserving `~/.ssh/config` parsing and rewriting.
//!
//! This crate has no GTK (or any other GUI toolkit) dependency: it exposes
//! [`ssh_config::SshConfig`] as the single entry point a frontend drives, plus the
//! toolkit-agnostic supporting types ([`metadata`], [`command`], [`watch`]). A future non-GTK
//! frontend could be built against this crate unchanged.

pub mod command;
pub mod metadata;
pub mod ssh_config;
pub mod watch;
