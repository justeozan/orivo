//! `orivo-plugin-sdk` — a plugin author's loop before there is a marketplace
//! to submit to: validate the manifest, check the component's contract, and
//! run one call against the real host on a fixture folder.
//!
//! No argument-parsing crate: three subcommands and a handful of `--flag
//! value` pairs do not earn a new dependency. See the SDK's `Cargo.toml` for
//! the same reasoning about `wit-parser`.

use orivo_plugin_sdk::{manifest_check, simulate};
use std::{path::PathBuf, process::ExitCode, time::Duration};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        print_usage();
        return ExitCode::FAILURE;
    };

    let result = match command.as_str() {
        "validate" => run_validate(args.collect()),
        "check" => run_check(args.collect()),
        "simulate" => run_simulate(args.collect()),
        "--help" | "-h" | "help" => {
            print_usage();
            return ExitCode::SUCCESS;
        }
        other => Err(format!("unknown command `{other}`")),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("orivo-plugin-sdk: {message}");
            ExitCode::FAILURE
        }
    }
}

fn print_usage() {
    eprintln!(
        "orivo-plugin-sdk <command>\n\n\
         Commands:\n  \
         validate <manifest.json | package-dir>   Validate a manifest, and the full\n                                            \
         package if the directory has a signature.\n  \
         check <package-dir>                       Run the pre-install contract and\n                                            \
         identity/health check the registry runs.\n  \
         simulate <package-dir> [options]          Run one call against the real host.\n\
         \n\
         simulate options:\n  \
         --request identity|health|validate-profile|discover-page|prepare-launch\n  \
         --grant NAME=PATH        (repeatable) a directory grant, e.g. fixture-games=./games\n  \
         --profile-id ID\n  \
         --display-name NAME       (validate-profile)\n  \
         --game-reference REF      (prepare-launch)\n  \
         --cursor CURSOR           (discover-page)\n  \
         --limit N                 (discover-page, default 20)\n  \
         --timeout-ms N            (default 5000)\n"
    );
}

fn run_validate(args: Vec<String>) -> Result<(), String> {
    let path = args
        .first()
        .ok_or("usage: orivo-plugin-sdk validate <manifest.json | package-dir>")?;
    let path = PathBuf::from(path);

    if path.is_dir() {
        match manifest_check::validate_package_dir(&path) {
            Ok(report) => {
                println!(
                    "OK  {} {} — package is valid",
                    report.manifest.id(),
                    report.manifest.manifest().version
                );
                for mismatch in &report.hash_mismatches {
                    println!("WARN  {mismatch}");
                }
                Ok(())
            }
            Err(errors) => Err(render_errors(&errors)),
        }
    } else {
        match manifest_check::validate_manifest_file(&path) {
            Ok(report) => {
                println!(
                    "OK  {} {} — manifest is valid (run `validate` on its directory, with a \
                     signature.ed25519 present, to also check packaging)",
                    report.manifest.id(),
                    report.manifest.manifest().version
                );
                Ok(())
            }
            Err(errors) => Err(render_errors(&errors)),
        }
    }
}

fn run_check(args: Vec<String>) -> Result<(), String> {
    let dir = args
        .first()
        .ok_or("usage: orivo-plugin-sdk check <package-dir>")?;
    let report = simulate::check_contract(&PathBuf::from(dir)).map_err(|errors| render_errors(&errors))?;

    println!("component contract: {:?}", report.contract);
    match report.health {
        Ok(Some(health)) => println!("identity + health: OK — {health:?}"),
        Ok(None) => println!("identity + health: not checked (contract-only)"),
        Err(message) => println!("identity + health: FAILED — {message}"),
    }
    Ok(())
}

fn run_simulate(args: Vec<String>) -> Result<(), String> {
    let mut dir = None;
    let mut request_kind = None;
    let mut grants = Vec::new();
    let mut profile_id = "sdk-profile".to_string();
    let mut display_name = "SDK Profile".to_string();
    let mut game_reference = "fixture:ok".to_string();
    let mut cursor = None;
    let mut limit = 20u32;
    let mut timeout_ms = 5_000u64;

    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        let mut value = || iter.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--request" => request_kind = Some(value()?),
            "--grant" => {
                let raw = value()?;
                let (name, path) = raw
                    .split_once('=')
                    .ok_or("--grant expects NAME=PATH")?;
                grants.push(simulate::SimulatedGrant {
                    name: name.to_string(),
                    path: PathBuf::from(path),
                });
            }
            "--profile-id" => profile_id = value()?,
            "--display-name" => display_name = value()?,
            "--game-reference" => game_reference = value()?,
            "--cursor" => cursor = Some(value()?),
            "--limit" => limit = value()?.parse().map_err(|_| "--limit expects a number")?,
            "--timeout-ms" => {
                timeout_ms = value()?.parse().map_err(|_| "--timeout-ms expects a number")?
            }
            other if dir.is_none() && !other.starts_with("--") => dir = Some(other.to_string()),
            other => return Err(format!("unrecognised argument `{other}`")),
        }
    }

    let dir = PathBuf::from(dir.ok_or("usage: orivo-plugin-sdk simulate <package-dir> [options]")?);
    let request = match request_kind.as_deref() {
        Some("identity") => simulate::SimulatedRequest::Identity,
        Some("health") => simulate::SimulatedRequest::HealthCheck,
        Some("validate-profile") => simulate::SimulatedRequest::ValidateProfile {
            profile_id,
            display_name,
        },
        Some("discover-page") => simulate::SimulatedRequest::DiscoverPage {
            profile_id,
            cursor,
            limit,
        },
        Some("prepare-launch") | None => simulate::SimulatedRequest::PrepareLaunch {
            profile_id,
            game_reference,
        },
        Some(other) => return Err(format!("unknown --request `{other}`")),
    };

    let report = simulate::simulate(&dir, &grants, request, Duration::from_millis(timeout_ms))
        .map_err(|errors| render_errors(&errors))?;

    match report.outcome {
        Ok(response) => println!("OK  {response:?}"),
        Err(message) => println!("FAILED  {message}"),
    }
    if !report.decisions.is_empty() {
        println!("\nhost decisions:");
        for entry in &report.decisions {
            println!("  [{}] {}: {}", entry.correlation_id, entry.decision, entry.detail);
        }
    }
    if !report.plugin_messages.is_empty() {
        println!("\nplugin log:");
        for entry in &report.plugin_messages {
            println!("  [{}] {}", entry.correlation_id, entry.detail);
        }
    }
    Ok(())
}

fn render_errors(errors: &[manifest_check::LocatedError]) -> String {
    errors
        .iter()
        .map(|error| error.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}
