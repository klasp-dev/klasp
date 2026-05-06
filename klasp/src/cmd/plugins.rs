//! `klasp plugins` — plugin management subcommands (list / info / disable).
//!
//! All three subcommands are read-only at v0.3. `enable` is explicitly out of
//! scope (re-enabling is done by removing from the disable list, which users
//! can do directly in the TOML file).
//!
//! Disable list: `$KLASP_DISABLED_PLUGINS_FILE` or
//! `~/.config/klasp/disabled-plugins.toml`. See `klasp_core::plugin_disable`.

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use klasp_core::{
    plugin_disable_add, plugin_disable_load, resolve_disable_list_path, validate_plugin_name,
    KLASP_PLUGIN_BIN_PREFIX,
};

use crate::cli::PluginsAction;
use crate::sources::plugin::fetch_describe_with_timeout;

/// Default per-binary timeout for `klasp plugins list`'s `--describe` calls.
/// Intentionally much shorter than the gate's 60 s plugin timeout — `list`
/// is interactive, and one hung plugin shouldn't make the whole listing
/// appear hung.
const DEFAULT_LIST_TIMEOUT_SECS: u64 = 5;

const NAME_COL: usize = 24;

pub fn run(action: &PluginsAction) -> ExitCode {
    match action {
        PluginsAction::List => cmd_list(),
        PluginsAction::Info { name } => cmd_info(name),
        PluginsAction::Disable { name } => cmd_disable(name),
    }
}

// ── list ─────────────────────────────────────────────────────────────────────

fn list_timeout() -> Duration {
    let secs = env::var("KLASP_PLUGIN_LIST_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_LIST_TIMEOUT_SECS);
    Duration::from_secs(secs)
}

/// Truncate `s` to fit `NAME_COL`, appending an ellipsis if it overflows.
fn fit_name(s: &str) -> String {
    if s.chars().count() > NAME_COL {
        let truncated: String = s.chars().take(NAME_COL - 1).collect();
        format!("{truncated}…")
    } else {
        s.to_string()
    }
}

/// Scan `$PATH` for `klasp-plugin-*` binaries and print a status table.
fn cmd_list() -> ExitCode {
    let disabled = plugin_disable_load(None);
    let plugins = scan_path_for_plugins();

    if plugins.is_empty() {
        eprintln!("No `{KLASP_PLUGIN_BIN_PREFIX}*` binaries found on $PATH.");
        return ExitCode::SUCCESS;
    }

    println!(
        "{:<width$} {:<10} STATUS",
        "NAME",
        "PROTOCOL",
        width = NAME_COL
    );

    let timeout = list_timeout();

    for bin_path in &plugins {
        let file_name = bin_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let name = file_name
            .strip_prefix(KLASP_PLUGIN_BIN_PREFIX)
            .unwrap_or(file_name);
        let display_name = fit_name(name);

        if disabled.contains(name) {
            // No describe spawn for disabled plugins — they're skipped at gate
            // time anyway, and a hung disabled plugin shouldn't slow `list`.
            println!(
                "{:<width$} {:<10} disabled",
                display_name,
                "—",
                width = NAME_COL,
            );
            continue;
        }

        match fetch_describe_with_timeout(bin_path, timeout) {
            Ok(desc) => {
                let status = if desc.protocol_version == klasp_core::PLUGIN_PROTOCOL_VERSION {
                    "enabled".to_string()
                } else {
                    format!(
                        "proto-mismatch (klasp speaks v{})",
                        klasp_core::PLUGIN_PROTOCOL_VERSION
                    )
                };
                println!(
                    "{:<width$} {:<10} {}",
                    display_name,
                    desc.protocol_version,
                    status,
                    width = NAME_COL,
                );
            }
            Err(reason) => {
                let short_reason = reason.lines().next().unwrap_or(&reason);
                println!(
                    "{:<width$} {:<10} describe-failed: {}",
                    display_name,
                    "—",
                    short_reason,
                    width = NAME_COL,
                );
            }
        }
    }

    ExitCode::SUCCESS
}

/// Walk each `$PATH` entry and collect binaries whose names start with
/// `klasp-plugin-`. Deduplicates by filename (first occurrence on PATH wins,
/// matching POSIX semantics).
fn scan_path_for_plugins() -> Vec<PathBuf> {
    let path_var = env::var("PATH").unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    let mut results = Vec::new();

    for dir in env::split_paths(&path_var) {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            if !name.starts_with(KLASP_PLUGIN_BIN_PREFIX) {
                continue;
            }
            if seen.insert(name) {
                results.push(path);
            }
        }
    }

    results
}

// ── info ─────────────────────────────────────────────────────────────────────

/// Run `--describe` for a named plugin and pretty-print the result. Does not
/// validate the protocol version — `info` is for inspecting any plugin,
/// including future-version ones the user might be debugging.
fn cmd_info(name: &str) -> ExitCode {
    if let Err(e) = validate_plugin_name(name) {
        eprintln!("error: {e}");
        return ExitCode::FAILURE;
    }

    let bin_name = format!("{KLASP_PLUGIN_BIN_PREFIX}{name}");
    let binary = match which::which(&bin_name) {
        Ok(p) => p,
        Err(_) => {
            eprintln!("error: `{bin_name}` not found on $PATH");
            return ExitCode::FAILURE;
        }
    };

    match fetch_describe_with_timeout(&binary, list_timeout()) {
        Ok(desc) => match serde_json::to_string_pretty(&desc) {
            Ok(pretty) => {
                println!("{pretty}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("error: could not serialize describe output: {e}");
                ExitCode::FAILURE
            }
        },
        Err(reason) => {
            eprintln!("error: {reason}");
            ExitCode::FAILURE
        }
    }
}

// ── disable ───────────────────────────────────────────────────────────────────

/// Add a plugin to the per-user disable list. Validates the name first so a
/// malicious or accidental string (path separators, shell metachars, control
/// chars) can't end up in the on-disk TOML.
fn cmd_disable(name: &str) -> ExitCode {
    if let Err(e) = validate_plugin_name(name) {
        eprintln!("error: {e}");
        return ExitCode::FAILURE;
    }

    let existing = plugin_disable_load(None);
    if existing.contains(name) {
        println!("{name} already disabled");
        return ExitCode::SUCCESS;
    }

    let path = resolve_disable_list_path();
    match plugin_disable_add(name, None) {
        Ok(()) => {
            println!(
                "disabled {name}; klasp gate will skip this plugin.\nDisable list: {}",
                path.display()
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: failed to update disable list: {e}");
            ExitCode::FAILURE
        }
    }
}
