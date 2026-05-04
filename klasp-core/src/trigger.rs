//! Trigger pattern matching for git commit/push.
//!
//! Design: [docs/design.md §6]. The regex is a Rust port of fallow's POSIX
//! ERE pattern, compiled once via `OnceLock`. Edge cases the regex
//! deliberately misses (`bash -c "git push"`, `eval "git commit"`,
//! `GIT_DIR=… git commit`, aliases like `gp`) are documented in design §6
//! and treated as non-goals for v0.1; klasp gates honest agents, not
//! adversarial ones.

use std::sync::OnceLock;

use regex::Regex;

/// The git event a tool-call command was classified as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitEvent {
    Commit,
    Push,
}

/// Stateless namespace for trigger classification. The regex itself is held
/// in a process-wide `OnceLock`.
pub struct Trigger;

fn pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Anchor on a non-word boundary so `forgit commit` and `mygit push`
        // don't match. The first capture group disambiguates commit vs push.
        Regex::new(r"(?:^|[\s;|&()])git\s+(commit|push)(?:\s|$)")
            .expect("trigger regex must compile")
    })
}

impl Trigger {
    /// Classify a shell command. Returns `Some(GitEvent)` if the command
    /// represents a git commit or push that should run the gate, `None`
    /// otherwise.
    pub fn classify(cmd: &str) -> Option<GitEvent> {
        let captures = pattern().captures(cmd)?;
        match captures.get(1)?.as_str() {
            "commit" => Some(GitEvent::Commit),
            "push" => Some(GitEvent::Push),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_bare_commit() {
        assert_eq!(Trigger::classify("git commit"), Some(GitEvent::Commit));
    }

    #[test]
    fn matches_commit_with_flags() {
        assert_eq!(
            Trigger::classify("git commit -m 'wip'"),
            Some(GitEvent::Commit),
        );
    }

    #[test]
    fn matches_bare_push() {
        assert_eq!(Trigger::classify("git push"), Some(GitEvent::Push));
    }

    #[test]
    fn matches_push_with_flags() {
        assert_eq!(
            Trigger::classify("git push origin main"),
            Some(GitEvent::Push),
        );
    }

    #[test]
    fn matches_chained_with_double_amp() {
        assert_eq!(
            Trigger::classify("cargo test && git push"),
            Some(GitEvent::Push),
        );
    }

    #[test]
    fn matches_chained_with_semicolon() {
        assert_eq!(
            Trigger::classify("cargo fmt; git commit"),
            Some(GitEvent::Commit),
        );
    }

    #[test]
    fn matches_chained_with_pipe() {
        assert_eq!(
            Trigger::classify("echo hi | git commit -F -"),
            Some(GitEvent::Commit),
        );
    }

    #[test]
    fn matches_subshell_parens() {
        // Subshell-grouped invocation: `(git commit ...)` is a common idiom
        // when chaining `cd dir && (git commit ...) && other`. The leading
        // `(` is in the boundary-character set so the regex anchors cleanly.
        assert_eq!(
            Trigger::classify("(git commit -m 'wip')"),
            Some(GitEvent::Commit),
        );
    }

    #[test]
    fn matches_subshell_push() {
        assert_eq!(
            Trigger::classify("(cd subdir && git push origin main)"),
            Some(GitEvent::Push),
        );
    }

    #[test]
    fn rejects_forgit() {
        assert_eq!(Trigger::classify("forgit commit"), None);
    }

    #[test]
    fn rejects_mygit() {
        assert_eq!(Trigger::classify("mygit push"), None);
    }

    #[test]
    fn rejects_committed_substring() {
        // Hypothetical command that mentions the substring "commit" but isn't
        // a git commit invocation.
        assert_eq!(Trigger::classify("git committed-files-tool"), None);
        // Bare `git committed` — the `(?:\s|$)` tail anchor must reject the
        // `t` after `commit` here too, not just whatever-follows-a-dash.
        assert_eq!(Trigger::classify("git committed"), None);
    }

    #[test]
    fn rejects_unrelated_git_subcommand() {
        assert_eq!(Trigger::classify("git status"), None);
        assert_eq!(Trigger::classify("git log"), None);
    }

    #[test]
    fn rejects_plain_text() {
        assert_eq!(Trigger::classify("ls -la"), None);
        assert_eq!(Trigger::classify(""), None);
    }

    /// Documented limitation: `git -c key=value commit` puts a flag between
    /// `git` and the subcommand, so the simple regex doesn't recognise it.
    /// Pinned as `#[ignore]` so the limitation lives in code; v0.2 may
    /// upgrade the trigger language and lift this. See [docs/design.md §6,
    /// §10].
    #[test]
    #[ignore = "design §6 known limitation; tracked for v0.2"]
    fn matches_git_dash_c_commit() {
        assert_eq!(
            Trigger::classify("git -c user.email=x@y.z commit"),
            Some(GitEvent::Commit),
        );
    }

    /// Deliberate non-goal per [docs/design.md §6]: a `bash -c "git push"`
    /// payload hides the trigger inside a quoted argument the regex never
    /// inspects. Honest agents don't do this; adversarial ones can bypass
    /// klasp trivially anyway (`bash -c "$(echo ... | base64 -d)"`).
    #[test]
    #[ignore = "design §6 deliberate non-goal; klasp gates honest agents"]
    fn deliberately_misses_bash_c_quoted() {
        assert_eq!(
            Trigger::classify(r#"bash -c "git push""#),
            Some(GitEvent::Push),
        );
    }

    /// Deliberate non-goal per [docs/design.md §6]: `eval` defers
    /// classification to a runtime-constructed string the regex can't see.
    #[test]
    #[ignore = "design §6 deliberate non-goal; klasp gates honest agents"]
    fn deliberately_misses_eval_quoted() {
        assert_eq!(
            Trigger::classify(r#"eval "git commit""#),
            Some(GitEvent::Commit),
        );
    }

    /// Deliberate non-goal per [docs/design.md §6]: env-var-prefixed
    /// invocations such as `GIT_DIR=/elsewhere git push` are uncommon outside
    /// scripts. v0.2 may add a leading-env-assignment skip.
    #[test]
    #[ignore = "design §6 deliberate non-goal; v0.2 candidate"]
    fn deliberately_misses_env_prefixed() {
        assert_eq!(
            Trigger::classify("GIT_DIR=/elsewhere git push"),
            Some(GitEvent::Push),
        );
    }

    /// Deliberate non-goal per [docs/design.md §6]: shell aliases such as
    /// `gp = git push` resolve at the shell layer. The regex inspects the
    /// raw tool-input command, not the post-alias-expansion form.
    #[test]
    #[ignore = "design §6 deliberate non-goal; shell aliases are out of scope"]
    fn deliberately_misses_alias() {
        assert_eq!(Trigger::classify("gp"), Some(GitEvent::Push));
    }
}
