//! Audit agentic-flow receipts and translate the result into plugin findings.
//!
//! This is a **read-only auditor**. It loads the flow.yaml manifest (source of
//! truth for step order/gating/enabled), reads `.agentic-flow/state.json` (the
//! cursor/index) and the per-step `.agentic-flow/receipts/NN-step.json` files
//! (the per-step source of truth), then reconciles the required steps for the
//! trigger depth against the receipts.
//!
//! Discipline mirrors the pre-commit reference plugin:
//! - `infra_warn(suffix, msg)` — one-finding `Warn` builder for plugin/infra
//!   errors. These NEVER produce a `fail` verdict.
//! - `MAX_FINDINGS` cap + truncation sentinel.
//! - protocol-version mismatch → best-effort warn finding (not fail).
//! - severity `"error"` → `Fail`, empty → `Pass`, else → `Warn`.
//!
//! The canonical `diff_hash` recipe lives in `docs/agentic-flow-receipts.md` and
//! is implemented byte-for-byte in [`canonical_diff_hash`]. The writer side (in
//! the agentic-flow orchestrator) MUST compute it identically.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

use crate::protocol::{
    PluginFinding, PluginGateInput, PluginGateOutput, PluginTriggerKind, PluginVerdict,
    PROTOCOL_VERSION,
};
use crate::receipt::{Manifest, Receipt, StateJson};

/// Rule slug prefix for infrastructure errors emitted by this plugin
/// (input-parse-error, output-serialize-failed, etc).
const HOOK_RULE_PREFIX_INFRA: &str = "klasp-plugin-agentic-flow/";
/// Rule slug for unknown-protocol warnings emitted by this plugin.
const PROTOCOL_WARN_RULE: &str = "klasp-plugin-agentic-flow/protocol-warn";
/// Rule slug for a state.json schema-version mismatch.
const STATE_VERSION_RULE: &str = "klasp-plugin-agentic-flow/state-version";

// ── Audit rule slugs (these can produce error → fail) ──────────────────────────
const RULE_MISSING: &str = "agentic-flow/missing-step";
const RULE_STALE: &str = "agentic-flow/stale-step";
const RULE_UNCONFIRMED: &str = "agentic-flow/unconfirmed-step";
const RULE_UNKNOWN: &str = "agentic-flow/unknown-step";

/// Maximum number of findings to emit before truncating.
const MAX_FINDINGS: usize = 100;

/// DoS guard: cap state/receipt/manifest reads at 8 MiB. Mirrors klasp's
/// output-cap philosophy — a hostile or runaway file must not be read unbounded
/// into memory. Anything larger than this is treated as an infra issue (warn).
const MAX_READ_BYTES: u64 = 8 * 1024 * 1024;

/// state.json schema version this plugin v1 understands.
const SUPPORTED_STATE_VERSION: u32 = 1;

/// Default manifest path (expanded for `~`).
const DEFAULT_MANIFEST: &str = "~/.claude/agentic-flow/flow.yaml";
/// Default state.json path (repo-relative).
const DEFAULT_STATE: &str = ".agentic-flow/state.json";
/// Default receipts dir (repo-relative).
const DEFAULT_RECEIPTS: &str = ".agentic-flow/receipts/";

/// The required-set of step ids for each trigger depth. These mirror the
/// agentic-flow flow.yaml ids. The plugin recognizes these natively; a manifest
/// step the plugin doesn't recognize produces a `warn` (not a fail).
const COMMIT_IMPL_STEPS: &[&str] = &["feature-dev", "dispatch-impl"];
const PUSH_REQUIRED_STEPS: &[&str] =
    &["simplify", "code-review", "review-handoff", "quality-gates"];
/// pr-create depth (only reachable via `settings.phase`, see ARCHITECTURAL note).
const PR_CREATE_REQUIRED_STEPS: &[&str] = &["triage-followups"];
/// pr-merge depth (only reachable via `settings.phase`).
const PR_MERGE_REQUIRED_STEPS: &[&str] = &["merge"];

/// All step ids this plugin v1 recognizes (for the unknown-step warning).
const KNOWN_STEP_IDS: &[&str] = &[
    "ideate",
    "graphify-onboard",
    "feature-dev",
    "log-issue",
    "dispatch-impl",
    "simplify",
    "code-review",
    "review-handoff",
    "quality-gates",
    "triage-followups",
    "merge",
    "schedule-routines",
];

/// Resolved audit depth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Commit,
    Push,
    PrCreate,
    PrMerge,
}

/// Run the gate check and produce a `PluginGateOutput`.
///
/// Never panics; all errors are captured as warn-level findings so the plugin
/// always exits 0 with well-formed JSON. The verdict is `Fail` only when an
/// *audit* rule (missing/stale/unconfirmed) produces an error finding.
pub fn run_gate(input: &PluginGateInput) -> PluginGateOutput {
    // Warn if the caller is on a newer protocol version but continue best-effort.
    let protocol_warn = if input.protocol_version != PROTOCOL_VERSION {
        Some(warn_finding(
            PROTOCOL_WARN_RULE,
            &format!(
                "received protocol_version={} but this plugin speaks v{}; \
                 proceeding best-effort — update the plugin when klasp v1.0 ships",
                input.protocol_version, PROTOCOL_VERSION
            ),
        ))
    } else {
        None
    };

    match audit(input) {
        Ok(mut findings) => {
            // Prepend any protocol warning so it's visible at the top.
            if let Some(pw) = protocol_warn {
                findings.insert(0, pw);
            }
            let findings = truncate_findings(findings);
            let verdict = verdict_for(&findings);
            PluginGateOutput {
                protocol_version: PROTOCOL_VERSION,
                verdict,
                findings,
            }
        }
        // Infrastructure errors NEVER fail — they warn. Fold in the protocol
        // warning if present so callers still see it.
        Err(mut warn) => {
            if let Some(pw) = protocol_warn {
                warn.findings.insert(0, pw);
            }
            warn
        }
    }
}

/// Map a findings list to a verdict: any `error` → Fail, empty → Pass, else Warn.
fn verdict_for(findings: &[PluginFinding]) -> PluginVerdict {
    if findings.iter().any(|f| f.severity == "error") {
        PluginVerdict::Fail
    } else if findings.is_empty() {
        PluginVerdict::Pass
    } else {
        PluginVerdict::Warn
    }
}

/// The core audit. Returns `Ok(findings)` for a normal audit (which may be
/// empty → pass) or `Err(PluginGateOutput::Warn)` for an infrastructure error
/// (missing dirs/files, malformed JSON/YAML, git failure) that must never fail.
fn audit(input: &PluginGateInput) -> Result<Vec<PluginFinding>, PluginGateOutput> {
    // SECURITY (up front, before any git runs): a base_ref shaped like a flag
    // (e.g. `--output=<path>`) would be honoured by `git diff` as an option
    // rather than a revision, letting a malicious base_ref write an
    // attacker-chosen file. Refuse it gracefully — infra warn, never a fail,
    // exit 0. `canonical_diff_hash` re-checks this as defense-in-depth.
    if input.base_ref.starts_with('-') {
        return Err(infra_warn(
            "base-ref-rejected",
            format!(
                "refusing base_ref starting with '-': {:?} (could be smuggled as a git flag)",
                input.base_ref
            ),
        ));
    }

    let repo_root = Path::new(&input.repo_root);
    let settings = input.config.settings.as_ref();

    // 1. Resolve paths from settings (with defaults).
    let manifest_path = resolve_manifest_path(settings);
    let state_path = resolve_repo_path(repo_root, settings, "state", DEFAULT_STATE);
    let receipts_dir = resolve_repo_path(repo_root, settings, "receipts", DEFAULT_RECEIPTS);

    // 2. Load the manifest (source of truth for order/gating/enabled).
    let manifest = load_manifest(&manifest_path)?;

    // 3. Determine the audit depth.
    let phase = resolve_phase(input.trigger.kind, settings);

    // 4. Load state.json (the cursor/index). Missing/malformed → infra warn.
    let state = load_state(&state_path)?;

    let mut findings: Vec<PluginFinding> = Vec::new();

    // state.json schema-version mismatch is advisory (warn), not fatal.
    if let Some(v) = state.version {
        if v != SUPPORTED_STATE_VERSION {
            findings.push(warn_finding(
                STATE_VERSION_RULE,
                &format!(
                    "state.json version {v} != supported version {SUPPORTED_STATE_VERSION}; \
                     auditing best-effort"
                ),
            ));
        }
    }

    // Build the NN-numbering + gating map from the manifest order (1-based).
    let positions = manifest_positions(&manifest);

    // Unknown-step warning: any manifest step the plugin doesn't recognize.
    for step in &manifest.steps {
        if let Some(id) = step.id.as_deref() {
            if step.enabled && !KNOWN_STEP_IDS.contains(&id) {
                findings.push(warn_finding(
                    RULE_UNKNOWN,
                    &format!(
                        "manifest step `{id}` is not recognized by this plugin's defaults; \
                         it is not enforced — update the plugin to audit it"
                    ),
                ));
            }
        }
    }

    // 5. Reconcile the required steps for this depth.
    let required = required_set(phase, &positions, &state);
    let receipts = load_receipts(&receipts_dir)?;

    // Surface malformed receipts as warn findings (infra). These never fail, but
    // the reconciliation also treats a malformed required receipt as absent so a
    // broken receipt cannot silently satisfy a required step.
    for (stem, parsed) in &receipts {
        if let Err(msg) = parsed {
            findings.push(warn_finding(
                &format!("{HOOK_RULE_PREFIX_INFRA}receipt-parse-error"),
                &format!("{stem}: {msg}"),
            ));
        }
    }

    // Each audit-error finding is paired with its NN so we can compute the
    // earliest failing step for the resume hint.
    let mut audit_errors: Vec<(u32, PluginFinding)> = Vec::new();

    for req in &required {
        match req {
            Requirement::Single(nn, id) => {
                let nn_step = format!("{nn:02}-{id}");
                let receipt_rel = receipt_rel_path(&receipts_dir, repo_root, &nn_step);
                if let StepResult::Error(f) = reconcile_step(
                    input,
                    &state,
                    &receipts,
                    &positions,
                    id,
                    &nn_step,
                    &receipt_rel,
                ) {
                    audit_errors.push((*nn, f));
                }
            }
            Requirement::OneOf(group_nn, alts) => {
                // Satisfied if ANY alternative reconciles OK.
                let any_ok = alts.iter().any(|(nn, id)| {
                    let nn_step = format!("{nn:02}-{id}");
                    let receipt_rel = receipt_rel_path(&receipts_dir, repo_root, &nn_step);
                    matches!(
                        reconcile_step(
                            input,
                            &state,
                            &receipts,
                            &positions,
                            id,
                            &nn_step,
                            &receipt_rel
                        ),
                        StepResult::Ok
                    )
                });
                // Point the finding at the earliest alternative's receipt.
                // `alts.first()` (vs `alts[0]`) can't panic if a future refactor
                // builds an empty OneOf — an empty group can't be unsatisfied, so
                // we simply skip it.
                if !any_ok {
                    if let Some((first_nn, first_id)) = alts.first() {
                        let labels: Vec<String> = alts
                            .iter()
                            .map(|(nn, id)| format!("{nn:02}-{id}"))
                            .collect();
                        let receipt_rel = receipt_rel_path(
                            &receipts_dir,
                            repo_root,
                            &format!("{first_nn:02}-{first_id}"),
                        );
                        audit_errors.push((
                            *group_nn,
                            error_finding(
                                RULE_MISSING,
                                Some(&receipt_rel),
                                &format!(
                                    "Missing required receipt for the impl path (need one of: {})",
                                    labels.join(" or ")
                                ),
                            ),
                        ));
                    }
                }
            }
        }
    }

    // 6. Inject the resume hint into the EARLIEST (min-NN) error finding's
    //    message so the joined Fail message leads with it.
    if !audit_errors.is_empty() {
        let min_nn = audit_errors.iter().map(|(nn, _)| *nn).min().unwrap_or(0);
        if let Some((_, lead)) = audit_errors.iter_mut().find(|(nn, _)| *nn == min_nn) {
            lead.message = format!(
                "{} Next: run /agentic-flow resume --from {min_nn:02}",
                lead.message
            );
        }
    }

    findings.extend(audit_errors.into_iter().map(|(_, f)| f));
    Ok(findings)
}

/// Per-step reconciliation outcome.
enum StepResult {
    Ok,
    Error(PluginFinding),
}

/// Reconcile a single required step. Implements the RECONCILIATION RULE and the
/// FRESHNESS / USER-CONFIRM checks from the RFC.
fn reconcile_step(
    input: &PluginGateInput,
    state: &StateJson,
    receipts: &BTreeMap<String, Result<Receipt, String>>,
    positions: &BTreeMap<String, PositionEntry>,
    id: &str,
    nn_step: &str,
    receipt_rel: &str,
) -> StepResult {
    let receipt = receipts.get(nn_step);

    // A malformed receipt is an infra issue surfaced as a warn finding inline,
    // but for the reconciliation we treat the step as if the receipt is absent
    // (so it does not silently pass). We still avoid a hard fail on parse error.
    let parsed: Option<&Receipt> = match receipt {
        Some(Ok(r)) => Some(r),
        Some(Err(_)) => None, // malformed → see warn emitted in load_receipts
        None => None,
    };

    // (a) completed + fresh → OK (subject to user-confirm).
    if let Some(r) = parsed {
        if r.is_completed() {
            // Freshness check.
            if let Some(reason) = staleness_reason(input, r) {
                return StepResult::Error(error_finding(
                    RULE_STALE,
                    Some(receipt_rel),
                    &format!("Receipt is stale: {reason} after {nn_step} completed"),
                ));
            }
            // User-confirm enforcement.
            let gating = manifest_gating(positions, id).or(r.gating.as_deref());
            if gating == Some("user-confirm") && !(r.user_confirmed && r.confirmation_id.is_some())
            {
                return StepResult::Error(error_finding(
                    RULE_UNCONFIRMED,
                    Some(receipt_rel),
                    &format!(
                        "user-confirm step {nn_step} has no user_confirmed=true \
                         (with a confirmation_id)"
                    ),
                ));
            }
            return StepResult::Ok;
        }
        // (b) status=skipped receipt → legitimately absent, OK.
        if r.is_skipped() {
            return StepResult::Ok;
        }
        // Any other status (e.g. "blocked") with no completed receipt is MISSING.
    }

    // (b) bare id present in state.json.skipped[] → legitimately absent, OK.
    if state.skipped.iter().any(|s| s == id) {
        return StepResult::Ok;
    }

    // (c) no completed/skipped receipt AND not in skipped[] → MISSING.
    StepResult::Error(error_finding(
        RULE_MISSING,
        Some(receipt_rel),
        &format!("Missing required receipt for {nn_step}"),
    ))
}

/// Determine why a completed receipt is stale, or `None` if it is fresh.
///
/// `diff_hash` is authoritative; `branch` mismatch also marks stale. `head`
/// mismatch alone (with a matching diff_hash) is fine (amended-but-identical).
fn staleness_reason(input: &PluginGateInput, r: &Receipt) -> Option<String> {
    // base_ref must match — a different comparison basis is stale.
    if let Some(rb) = r.base_ref.as_deref() {
        if rb != input.base_ref {
            return Some(format!(
                "receipt base_ref `{rb}` != gate base_ref `{}` (different comparison basis); diff changed",
                input.base_ref
            ));
        }
    }

    // branch mismatch → stale.
    if let Some(rbr) = r.branch.as_deref() {
        if let Some(cur) = current_branch(&input.repo_root) {
            if rbr != cur {
                return Some(format!(
                    "receipt branch `{rbr}` != current branch `{cur}`; diff changed"
                ));
            }
        }
    }

    // Authoritative freshness: recompute the canonical diff hash and compare.
    let current = match canonical_diff_hash(&input.repo_root, &input.base_ref, input.trigger.kind) {
        Ok(h) => h,
        // If we can't compute the hash, we can't prove staleness — treat as an
        // infra issue but do NOT fail here; bubble up as a warn elsewhere. We
        // return None so the step isn't marked stale on a git failure.
        Err(_) => return None,
    };
    match r.diff_hash.as_deref() {
        Some(h) if h == current => None,
        _ => Some("diff changed".to_string()),
    }
}

/// Compute the canonical diff hash exactly as documented in
/// `docs/agentic-flow-receipts.md`. **Writer and auditor MUST agree byte-for-byte.**
///
/// Primary component (both triggers):
/// `sha256( git -C <repo_root> diff --no-color --no-ext-diff <base_ref>...HEAD )`
///
/// On the COMMIT trigger ALSO fold in the staged delta:
/// `sha256( git -C <repo_root> diff --no-color --no-ext-diff --cached )`
/// and hash the concatenation of the two raw outputs.
///
/// Pinned: `--no-color --no-ext-diff`, three-dot (`...`) merge-base range,
/// default rename detection, raw bytes (NO trailing-newline trimming).
///
/// Returns `Ok("sha256:<hex>")` or `Err` if any `git diff` fails/non-zero.
pub fn canonical_diff_hash(
    repo_root: &str,
    base_ref: &str,
    kind: PluginTriggerKind,
) -> anyhow::Result<String> {
    // SECURITY: refuse a base_ref that looks like a flag. git parses options
    // *before* the `--` separator, so a base_ref like `--output=<path>` would be
    // honoured as the `--output` flag (range token = `--output=<path>...HEAD`),
    // letting a malicious klasp.toml / KLASP_BASE_REF write an attacker-chosen
    // file. The trailing `--` below does NOT defend against this (it only ends
    // option parsing for pathspecs); the up-front leading-dash reject does.
    if base_ref.starts_with('-') {
        anyhow::bail!("refusing base_ref starting with '-': {base_ref:?}");
    }

    let range = format!("{base_ref}...HEAD");
    // The trailing `--` end-of-options separator stops git from interpreting a
    // base_ref that resolves to a path/filename as a pathspec or flag.
    let three_dot = run_git_diff(
        repo_root,
        &["diff", "--no-color", "--no-ext-diff", &range, "--"],
    )?;

    let mut hasher = Sha256::new();
    hasher.update(&three_dot);

    if kind == PluginTriggerKind::Commit {
        let cached = run_git_diff(
            repo_root,
            &["diff", "--no-color", "--no-ext-diff", "--cached", "--"],
        )?;
        hasher.update(&cached);
    }

    let digest = hasher.finalize();
    Ok(format!("sha256:{}", hex_lower(&digest)))
}

/// Run `git -C <repo_root> <args...>` and return raw stdout bytes, erroring on a
/// non-zero exit or spawn failure.
fn run_git_diff(repo_root: &str, args: &[&str]) -> anyhow::Result<Vec<u8>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(args)
        .output()
        .map_err(|e| anyhow::anyhow!("failed to spawn git: {e}"))?;
    if !out.status.success() {
        anyhow::bail!(
            "git {} failed (status {:?}): {}",
            args.join(" "),
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

/// Lowercase hex encoding of a byte slice.
fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Resolve the current git branch name, or `None` if it can't be determined.
fn current_branch(repo_root: &str) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

// ── Path resolution ─────────────────────────────────────────────────────────
//
// TRUST MODEL: the settings paths (manifest/state/receipts) are trusted at the
// same level as `klasp.toml` — whoever can edit `klasp.toml` chooses where this
// plugin reads from. Review `klasp.toml` changes from untrusted contributors,
// since a hostile path here points the auditor at attacker-controlled files.

/// Read a string field from `settings`.
fn settings_str<'a>(settings: Option<&'a serde_json::Value>, key: &str) -> Option<&'a str> {
    settings?.get(key)?.as_str()
}

/// Resolve the manifest path from settings (expanding a leading `~`).
fn resolve_manifest_path(settings: Option<&serde_json::Value>) -> PathBuf {
    let raw = settings_str(settings, "manifest").unwrap_or(DEFAULT_MANIFEST);
    expand_tilde(raw)
}

/// Resolve a repo-relative settings path (state/receipts) against `repo_root`.
fn resolve_repo_path(
    repo_root: &Path,
    settings: Option<&serde_json::Value>,
    key: &str,
    default: &str,
) -> PathBuf {
    let raw = settings_str(settings, key).unwrap_or(default);
    let p = expand_tilde(raw);
    if p.is_absolute() {
        p
    } else {
        repo_root.join(p)
    }
}

/// Expand a leading `~` to the user's home directory.
fn expand_tilde(raw: &str) -> PathBuf {
    if let Some(rest) = raw.strip_prefix("~/") {
        if let Some(home) = home_dir() {
            return home.join(rest);
        }
    } else if raw == "~" {
        if let Some(home) = home_dir() {
            return home;
        }
    }
    PathBuf::from(raw)
}

/// Best-effort home directory from `$HOME`.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

// ── Phase / required-set ─────────────────────────────────────────────────────

/// Resolve the audit depth. Wire trigger maps to commit/push depth; the optional
/// `settings.phase` escape hatch can pin a deeper depth (see ARCHITECTURAL note
/// in README: protocol v0 cannot transmit pr-create/pr-merge as distinct triggers).
fn resolve_phase(kind: PluginTriggerKind, settings: Option<&serde_json::Value>) -> Phase {
    if let Some(p) = settings_str(settings, "phase") {
        match p {
            "commit" => return Phase::Commit,
            "push" => return Phase::Push,
            "pr-create" => return Phase::PrCreate,
            "pr-merge" => return Phase::PrMerge,
            _ => {} // unknown phase override → fall through to wire kind
        }
    }
    match kind {
        PluginTriggerKind::Commit => Phase::Commit,
        PluginTriggerKind::Push => Phase::Push,
    }
}

/// A single requirement in the required-set. `Single` must be satisfied on its
/// own; `OneOf` is satisfied if ANY of its alternatives is satisfied (used for
/// the commit-depth "feature-dev OR dispatch-impl" rule).
enum Requirement {
    /// `(NN, id)` — a single required step.
    Single(u32, String),
    /// `(NN, ids)` — satisfied if any alternative is. NN is the earliest of the
    /// group (used for the resume hint and finding ordering).
    OneOf(u32, Vec<(u32, String)>),
}

impl Requirement {
    /// The NN used for ordering / resume-hint computation.
    fn nn(&self) -> u32 {
        match self {
            Requirement::Single(nn, _) => *nn,
            Requirement::OneOf(nn, _) => *nn,
        }
    }
}

/// Build the ordered required-set for a phase.
///
/// - commit: `OneOf(feature-dev, dispatch-impl)` (the impl path must be reached)
///   PLUS every enabled `user-confirm` step up to `current_step`.
/// - push: simplify, code-review, review-handoff, quality-gates.
/// - pr-create / pr-merge (only via `settings.phase`): cumulative on push.
///
/// Disabled-in-manifest steps are dropped. NN is the 1-based manifest position
/// so the rendered receipt path matches flow.yaml ordering.
fn required_set(
    phase: Phase,
    positions: &BTreeMap<String, PositionEntry>,
    state: &StateJson,
) -> Vec<Requirement> {
    match phase {
        Phase::Commit => commit_required(positions, state),
        Phase::Push => singles(PUSH_REQUIRED_STEPS, positions),
        Phase::PrCreate => singles(
            &concat_steps(&[PUSH_REQUIRED_STEPS, PR_CREATE_REQUIRED_STEPS]),
            positions,
        ),
        Phase::PrMerge => singles(
            &concat_steps(&[
                PUSH_REQUIRED_STEPS,
                PR_CREATE_REQUIRED_STEPS,
                PR_MERGE_REQUIRED_STEPS,
            ]),
            positions,
        ),
    }
}

/// Flatten several `&[&str]` slices into one owned `Vec<&str>`.
fn concat_steps<'a>(groups: &[&'a [&'a str]]) -> Vec<&'a str> {
    groups.iter().flat_map(|g| g.iter().copied()).collect()
}

/// Turn a list of step ids into `Single` requirements, dropping disabled steps,
/// sorted by NN.
fn singles(ids: &[&str], positions: &BTreeMap<String, PositionEntry>) -> Vec<Requirement> {
    let mut out: Vec<Requirement> = Vec::new();
    for id in ids {
        if let Some(e) = positions.get(*id) {
            if !e.enabled {
                continue;
            }
        }
        let nn = nn_for(id, positions);
        out.push(Requirement::Single(nn, id.to_string()));
    }
    out.sort_by_key(|r| r.nn());
    out
}

/// The 1-based NN for a step id, from the manifest or the canonical fallback.
fn nn_for(id: &str, positions: &BTreeMap<String, PositionEntry>) -> u32 {
    positions
        .get(id)
        .map(|e| e.position)
        .unwrap_or_else(|| fallback_nn(id))
}

/// Fallback NN if a step id is missing from the manifest (uses the canonical
/// agentic-flow ordering). Keeps rendered receipt paths sensible even when the
/// manifest is sparse.
fn fallback_nn(id: &str) -> u32 {
    KNOWN_STEP_IDS
        .iter()
        .position(|k| *k == id)
        .map(|i| (i + 1) as u32)
        .unwrap_or(0)
}

/// The commit-depth required-set: the impl path (feature-dev OR dispatch-impl)
/// plus every enabled `user-confirm` step at or before `current_step`.
fn commit_required(
    positions: &BTreeMap<String, PositionEntry>,
    state: &StateJson,
) -> Vec<Requirement> {
    let mut out: Vec<Requirement> = Vec::new();

    // Impl path: OneOf(feature-dev, dispatch-impl), dropping any disabled ones.
    let impl_alts: Vec<(u32, String)> = COMMIT_IMPL_STEPS
        .iter()
        .filter(|id| positions.get(**id).map(|e| e.enabled).unwrap_or(true))
        .map(|id| (nn_for(id, positions), id.to_string()))
        .collect();
    if let Some(min_nn) = impl_alts.iter().map(|(nn, _)| *nn).min() {
        out.push(Requirement::OneOf(min_nn, impl_alts));
    }

    // Every enabled user-confirm step up to and including the cursor.
    let cursor_nn = state
        .current_step
        .as_deref()
        .map(|id| nn_for(id, positions))
        .unwrap_or(u32::MAX);
    for (sid, entry) in positions.iter() {
        if entry.enabled
            && entry.gating.as_deref() == Some("user-confirm")
            && entry.position <= cursor_nn
        {
            out.push(Requirement::Single(entry.position, sid.clone()));
        }
    }

    out.sort_by_key(|r| r.nn());
    out
}

// ── Manifest helpers ─────────────────────────────────────────────────────────

/// A manifest position entry: 1-based NN, enabled flag, gating.
#[derive(Debug, Clone)]
struct PositionEntry {
    position: u32,
    enabled: bool,
    gating: Option<String>,
}

/// Map step id → its 1-based manifest position, enabled flag and gating.
fn manifest_positions(manifest: &Manifest) -> BTreeMap<String, PositionEntry> {
    let mut map = BTreeMap::new();
    for (i, step) in manifest.steps.iter().enumerate() {
        if let Some(id) = step.id.as_deref() {
            map.insert(
                id.to_string(),
                PositionEntry {
                    position: (i + 1) as u32,
                    enabled: step.enabled,
                    gating: step.gating.clone(),
                },
            );
        }
    }
    map
}

/// The manifest gating for a step id, if present.
fn manifest_gating<'a>(
    positions: &'a BTreeMap<String, PositionEntry>,
    id: &str,
) -> Option<&'a str> {
    positions.get(id).and_then(|e| e.gating.as_deref())
}

// ── Loading ──────────────────────────────────────────────────────────────────

/// A capped-read error: either the file exceeded [`MAX_READ_BYTES`] or a plain
/// I/O error occurred. Both are infra issues (graceful warn), but they carry
/// distinct messages.
enum CappedReadErr {
    /// File is larger than [`MAX_READ_BYTES`].
    TooLarge,
    /// Open/read/utf-8 failure, with a human-readable description.
    Io(String),
}

/// Read a file to a `String` but never read more than [`MAX_READ_BYTES`].
///
/// DoS guard mirroring klasp's output-cap philosophy: open the file, `take()`
/// the cap+1 bytes, and reject (rather than read unbounded) if it overflows.
fn read_to_string_capped(path: &Path) -> Result<String, CappedReadErr> {
    let file = std::fs::File::open(path).map_err(|e| CappedReadErr::Io(e.to_string()))?;
    // Read one byte past the cap so we can detect overflow deterministically.
    let mut handle = file.take(MAX_READ_BYTES + 1);
    let mut buf = Vec::new();
    handle
        .read_to_end(&mut buf)
        .map_err(|e| CappedReadErr::Io(e.to_string()))?;
    if buf.len() as u64 > MAX_READ_BYTES {
        return Err(CappedReadErr::TooLarge);
    }
    String::from_utf8(buf).map_err(|e| CappedReadErr::Io(e.to_string()))
}

/// Load and parse flow.yaml. Unreadable/oversize/malformed → infra warn (never fail).
fn load_manifest(path: &Path) -> Result<Manifest, PluginGateOutput> {
    let text = read_to_string_capped(path).map_err(|e| match e {
        CappedReadErr::TooLarge => infra_warn(
            "manifest-too-large",
            format!(
                "flow.yaml at {} exceeds the {MAX_READ_BYTES}-byte read cap; refusing to read",
                path.display()
            ),
        ),
        CappedReadErr::Io(msg) => infra_warn(
            "manifest-unreadable",
            format!("cannot read flow.yaml at {}: {msg}", path.display()),
        ),
    })?;
    serde_yaml_ng::from_str::<Manifest>(&text).map_err(|e| {
        infra_warn(
            "manifest-parse-error",
            format!("flow.yaml at {} is malformed: {e}", path.display()),
        )
    })
}

/// Load and parse state.json. Missing/oversize/malformed → infra warn (never fail).
fn load_state(path: &Path) -> Result<StateJson, PluginGateOutput> {
    let text = read_to_string_capped(path).map_err(|e| match e {
        CappedReadErr::TooLarge => infra_warn(
            "state-too-large",
            format!(
                "state.json at {} exceeds the {MAX_READ_BYTES}-byte read cap; refusing to read",
                path.display()
            ),
        ),
        CappedReadErr::Io(msg) => infra_warn(
            "state-unreadable",
            format!("cannot read state.json at {}: {msg}", path.display()),
        ),
    })?;
    serde_json::from_str::<StateJson>(&text).map_err(|e| {
        infra_warn(
            "state-parse-error",
            format!("state.json at {} is malformed: {e}", path.display()),
        )
    })
}

/// Load all `NN-*.json` receipts in the receipts dir, keyed by `"NN-id"` stem.
///
/// A missing receipts directory is an infra error → warn (never fail). A single
/// malformed receipt is kept as `Err(msg)` in the map so reconcile_step can both
/// (a) treat the step as absent and (b) surface a warn finding inline.
fn load_receipts(
    dir: &Path,
) -> Result<BTreeMap<String, Result<Receipt, String>>, PluginGateOutput> {
    let entries = std::fs::read_dir(dir).map_err(|e| {
        infra_warn(
            "receipts-dir-missing",
            format!("cannot read receipts dir at {}: {e}", dir.display()),
        )
    })?;

    let mut map: BTreeMap<String, Result<Receipt, String>> = BTreeMap::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        let parsed = match read_to_string_capped(&path) {
            Ok(text) => serde_json::from_str::<Receipt>(&text)
                .map_err(|e| format!("malformed receipt {}: {e}", path.display())),
            // Oversize/unreadable receipts are surfaced as a parse-error warn and
            // treated as absent (a broken receipt can't silently satisfy a step).
            Err(CappedReadErr::TooLarge) => Err(format!(
                "receipt {} exceeds the {MAX_READ_BYTES}-byte read cap; treated as absent",
                path.display()
            )),
            Err(CappedReadErr::Io(msg)) => {
                Err(format!("unreadable receipt {}: {msg}", path.display()))
            }
        };
        map.insert(stem, parsed);
    }
    Ok(map)
}

/// Receipt path relative to repo_root for use in findings (best-effort; falls
/// back to the absolute receipt path if it can't be relativized).
fn receipt_rel_path(receipts_dir: &Path, repo_root: &Path, nn_step: &str) -> String {
    let abs = receipts_dir.join(format!("{nn_step}.json"));
    match abs.strip_prefix(repo_root) {
        Ok(rel) => rel.to_string_lossy().into_owned(),
        Err(_) => abs.to_string_lossy().into_owned(),
    }
}

// ── Findings ─────────────────────────────────────────────────────────────────

/// Cap findings at `MAX_FINDINGS` and append a sentinel if truncated.
fn truncate_findings(mut findings: Vec<PluginFinding>) -> Vec<PluginFinding> {
    if findings.len() <= MAX_FINDINGS {
        return findings;
    }
    findings.truncate(MAX_FINDINGS);
    findings.push(warn_finding(
        "klasp-plugin-agentic-flow/truncated",
        &format!(
            "additional findings not shown (truncated at {MAX_FINDINGS}); \
             re-run /agentic-flow status locally for the full list"
        ),
    ));
    findings
}

fn error_finding(rule: &str, file: Option<&str>, message: &str) -> PluginFinding {
    PluginFinding {
        severity: "error".to_string(),
        rule: rule.to_string(),
        file: file.map(|s| s.to_string()),
        line: None,
        message: message.to_string(),
    }
}

fn warn_finding(rule: &str, message: &str) -> PluginFinding {
    PluginFinding {
        severity: "warn".to_string(),
        rule: rule.to_string(),
        file: None,
        line: None,
        message: message.to_string(),
    }
}

/// Build a single-finding `Verdict::Warn` `PluginGateOutput` for plugin
/// infrastructure errors (input parse failure, internal serialization error,
/// missing dirs, malformed JSON/YAML, git failure, etc). Callers exit 0 after
/// writing this — the gate must continue running other checks even when this
/// plugin's input or environment is broken.
pub fn infra_warn(rule_suffix: &str, message: impl Into<String>) -> PluginGateOutput {
    PluginGateOutput {
        protocol_version: PROTOCOL_VERSION,
        verdict: PluginVerdict::Warn,
        findings: vec![warn_finding(
            &format!("{HOOK_RULE_PREFIX_INFRA}{rule_suffix}"),
            &message.into(),
        )],
    }
}
