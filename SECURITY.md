# Security Policy

## Supported versions

klasp is pre-1.0 and ships fixes against the latest released minor. Security
fixes are issued on the most recent `0.x` line only.

| Version | Supported          |
| ------- | ------------------ |
| 0.5.x   | :white_check_mark: |
| < 0.5.0 | :x: (please upgrade) |

## Reporting a vulnerability

Please report security issues **privately** — do not open a public issue for a
suspected vulnerability.

- Preferred: GitHub **private vulnerability reporting** —
  [open a report](https://github.com/klasp-dev/klasp/security/advisories/new)
  (repository **Security** tab → **Report a vulnerability**).

What to include: affected version, a minimal reproduction, and the impact you
observed. We aim to acknowledge a report within **3 business days** and to agree
on a disclosure timeline from there. We'll credit reporters who want it once a
fix ships.

## Security model — what klasp is and isn't

klasp is a **quality gate at the agent's tool-call surface and at git hooks**.
It runs your configured checks (fmt / lint / type-check / test) and returns a
structured pass/fail verdict so a cooperating AI coding agent self-corrects
before a commit or push lands.

Please calibrate expectations accordingly:

- **klasp is not an OS sandbox or a containment boundary.** It gates the
  surfaces it installs into (e.g. Claude Code's `PreToolUse` hook, the git
  `pre-commit` / `pre-push` hooks, the managed agent-config blocks). An agent
  or process that bypasses those surfaces — invoking git with `--no-verify`,
  shelling out to a raw `git` it controls, or writing files directly — is
  outside klasp's enforcement. klasp raises the floor on a cooperative agent;
  it is **not** a control against a hostile one.
- **Fail-open by default.** On a config parse error, version skew, or an
  internal error, klasp degrades to *allow* (with a notice on the surface) so it
  never silently wedges your commits. This is a deliberate availability choice
  and means klasp must **not** be relied on as a hard enforcement control in its
  default configuration. A fail-closed / enforce mode is tracked for a future
  release.
- **Plugins are subprocesses with your privileges.** The v0 subprocess plugin
  protocol runs plugin binaries you configure, as you, with your environment.
  Only install plugins you trust, the same as any other dev dependency.

## Scope

In scope: the `klasp` CLI and crates, hook/managed-block installation and
uninstallation, the gate runner, and parsing of plugin protocol I/O and
`klasp.toml`.

Out of scope: vulnerabilities in the underlying check tools klasp invokes
(`cargo`, `pytest`, your linter, etc.), in third-party plugins, or in the AI
agents themselves — please report those to their respective projects.
