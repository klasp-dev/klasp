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

use klasp_core::{
    plugin_disable_add, plugin_disable_load, resolve_disable_list_path, PluginDescribe,
    KLASP_PLUGIN_BIN_PREFIX,
};

use crate::cli::PluginsAction;
use crate::sources::plugin::fetch_describe;

pub fn run(action: &PluginsAction) -> ExitCode {
    match action {
        PluginsAction::List => cmd_list(),
        PluginsAction::Info { name } => cmd_info(name),
        PluginsAction::Disable { name } => cmd_disable(name),
    }
}

// ── list ─────────────────────────────────────────────────────────────────────

/// Scan `$PATH` for `klasp-plugin-*` binaries and print a status table.
fn cmd_list() -> ExitCode {
    let disabled = plugin_disable_load(None);
    let plugins = scan_path_for_plugins();

    if plugins.is_empty() {
        eprintln!("No klasp-plugin-* binaries found on $PATH.");
        return ExitCode::SUCCESS;
    }

    // Header
    println!("{:<20} {:<10} {:<10} STATUS", "NAME", "VERSION", "PROTOCOL");

    for bin_path in &plugins {
        let file_name = bin_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let name = file_name
            .strip_prefix(KLASP_PLUGIN_BIN_PREFIX)
            .unwrap_or(file_name);

        if disabled.contains(name) {
            // Still call --describe to get version info, but tag as disabled.
            let (version, protocol) = describe_version_fields(bin_path);
            println!("{:<20} {:<10} {:<10} disabled", name, version, protocol);
            continue;
        }

        match fetch_describe(bin_path) {
            Ok(desc) => {
                let version = desc
                    .name
                    .strip_prefix(KLASP_PLUGIN_BIN_PREFIX)
                    .map(|_| "—".to_string()) // name carries no semver in v0
                    .unwrap_or_else(|| "—".to_string());
                println!(
                    "{:<20} {:<10} {:<10} enabled",
                    name, version, desc.protocol_version,
                );
            }
            Err(reason) => {
                println!("{:<20} {:<10} {:<10} {}", name, "—", "—", reason);
            }
        }
    }

    ExitCode::SUCCESS
}

/// Call `--describe` and return (version_str, protocol_str) for display only.
fn describe_version_fields(bin_path: &std::path::Path) -> (String, String) {
    match fetch_describe(bin_path) {
        Ok(desc) => ("—".to_string(), desc.protocol_version.to_string()),
        Err(_) => ("—".to_string(), "—".to_string()),
    }
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

/// Run `--describe` for a named plugin and pretty-print the result.
fn cmd_info(name: &str) -> ExitCode {
    let bin_name = format!("{KLASP_PLUGIN_BIN_PREFIX}{name}");
    let binary = match which::which(&bin_name) {
        Ok(p) => p,
        Err(_) => {
            eprintln!("error: `{bin_name}` not found on $PATH");
            return ExitCode::FAILURE;
        }
    };

    match fetch_describe_raw(&binary) {
        Ok(desc) => {
            match serde_json::to_string_pretty(&desc) {
                Ok(pretty) => println!("{pretty}"),
                Err(e) => {
                    eprintln!("error: could not serialize describe output: {e}");
                    return ExitCode::FAILURE;
                }
            }
            ExitCode::SUCCESS
        }
        Err(reason) => {
            eprintln!("error: {reason}");
            ExitCode::FAILURE
        }
    }
}

/// Run `--describe` and return the raw `PluginDescribe` without protocol
/// version validation (so `info` shows what the plugin actually reports, even
/// if it's a future/incompatible version).
fn fetch_describe_raw(binary: &std::path::Path) -> Result<PluginDescribe, String> {
    use std::process::{Command, Stdio};

    let output = Command::new(binary)
        .arg("--describe")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("failed to spawn `{}`: {e}", binary.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "`{}` --describe exited {}: {}",
            binary.display(),
            output.status.code().unwrap_or(-1),
            stderr.trim(),
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(stdout.as_ref())
        .map_err(|e| format!("--describe produced malformed JSON: {e}"))
}

// ── disable ───────────────────────────────────────────────────────────────────

/// Add a plugin to the per-user disable list.
fn cmd_disable(name: &str) -> ExitCode {
    // Check if already disabled.
    let existing = plugin_disable_load(None);
    if existing.contains(name) {
        println!("{name} already disabled");
        return ExitCode::SUCCESS;
    }

    let path = resolve_disable_list_path();
    match plugin_disable_add(name, None) {
        Ok(()) => {
            println!(
                "disabled {name}; klasp gate will skip this plugin.\n\
                 Disable list: {}",
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
