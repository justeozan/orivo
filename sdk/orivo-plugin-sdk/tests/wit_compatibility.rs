//! Freezes `orivo-plugin@1` and fails when a change to `wit/orivo-plugin.wit`
//! would break an existing component built against it.
//!
//! The canonical shape below is keyed by name, not by `wit-parser`'s arena
//! indices — an additive change (P1's `host-files`/`host-journal` imports)
//! inserts new interfaces and would shift every later index, which would make
//! an index-keyed diff scream about changes that never happened. Naming
//! things instead is what lets the comparison say "the world still exports
//! `runner`" and mean it regardless of parse order.
//!
//! What counts as breaking, and why:
//!   * a named type disappearing, or changing its recorded shape (a field,
//!     variant or case added/removed/renamed) — a component compiled against
//!     the old shape reads or writes the wrong bytes under the new one;
//!   * an interface function disappearing, or changing its signature — same
//!     reason, at the ABI boundary instead of inside a record;
//!   * a world losing an export or an import it used to have.
//! Anything *new* — a type, a function, an interface, an import — is not
//! flagged: an old component simply never uses it, which is the whole reason
//! `host-files`/`host-journal` could land inside v1 instead of forcing a v2.

use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};
use wit_parser::{Resolve, Type, TypeDefKind, TypeOwner, WorldKey};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct WorldSnapshot {
    imports: BTreeSet<String>,
    exports: BTreeSet<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Snapshot {
    /// `"interface-name.type-name"` -> a canonical rendering of its shape.
    types: BTreeMap<String, String>,
    /// `"interface-name"` -> `"function-name"` -> `"(params) -> result"`.
    interfaces: BTreeMap<String, BTreeMap<String, String>>,
    /// `"world-name"` -> the interfaces it imports/exports, by name.
    worlds: BTreeMap<String, WorldSnapshot>,
}

fn snapshot(resolve: &Resolve) -> Snapshot {
    let mut types = BTreeMap::new();
    let mut interfaces = BTreeMap::new();
    for (_, iface) in resolve.interfaces.iter() {
        let Some(iface_name) = &iface.name else {
            continue;
        };
        for (type_name, &type_id) in &iface.types {
            types.insert(
                format!("{iface_name}.{type_name}"),
                render_typedef(resolve, &resolve.types[type_id]),
            );
        }
        let mut functions = BTreeMap::new();
        for (fn_name, function) in &iface.functions {
            let params = function
                .params
                .iter()
                .map(|param| format!("{}:{}", param.name, render_type(resolve, &param.ty)))
                .collect::<Vec<_>>()
                .join(",");
            let result = function
                .result
                .map(|ty| render_type(resolve, &ty))
                .unwrap_or_else(|| "()".to_string());
            functions.insert(fn_name.clone(), format!("({params})->{result}"));
        }
        interfaces.insert(iface_name.clone(), functions);
    }

    let mut worlds = BTreeMap::new();
    for (_, world) in resolve.worlds.iter() {
        worlds.insert(
            world.name.clone(),
            WorldSnapshot {
                imports: world_item_names(resolve, world.imports.keys()),
                exports: world_item_names(resolve, world.exports.keys()),
            },
        );
    }

    Snapshot {
        types,
        interfaces,
        worlds,
    }
}

fn world_item_names<'a>(
    resolve: &Resolve,
    keys: impl Iterator<Item = &'a WorldKey>,
) -> BTreeSet<String> {
    keys.map(|key| world_key_name(resolve, key)).collect()
}

fn world_key_name(resolve: &Resolve, key: &WorldKey) -> String {
    match key {
        WorldKey::Name(name) => name.clone(),
        WorldKey::Interface(id) => resolve.interfaces[*id]
            .name
            .clone()
            .unwrap_or_else(|| format!("interface-{}", id.index())),
    }
}

fn render_type(resolve: &Resolve, ty: &Type) -> String {
    match ty {
        Type::Bool => "bool".into(),
        Type::U8 => "u8".into(),
        Type::U16 => "u16".into(),
        Type::U32 => "u32".into(),
        Type::U64 => "u64".into(),
        Type::S8 => "s8".into(),
        Type::S16 => "s16".into(),
        Type::S32 => "s32".into(),
        Type::S64 => "s64".into(),
        Type::F32 => "f32".into(),
        Type::F64 => "f64".into(),
        Type::Char => "char".into(),
        Type::String => "string".into(),
        Type::ErrorContext => "error-context".into(),
        Type::Id(id) => {
            let def = &resolve.types[*id];
            match &def.name {
                // A named type is referenced by its qualified name; its own
                // shape is recorded once, under that same name, in `types`.
                Some(name) => qualify(resolve, def.owner, name),
                None => render_typedef(resolve, def),
            }
        }
    }
}

fn qualify(resolve: &Resolve, owner: TypeOwner, name: &str) -> String {
    match owner {
        TypeOwner::Interface(id) => {
            let iface = resolve.interfaces[id]
                .name
                .clone()
                .unwrap_or_else(|| format!("interface-{}", id.index()));
            format!("{iface}.{name}")
        }
        TypeOwner::World(id) => format!("world-{}.{name}", id.index()),
        TypeOwner::None => name.to_string(),
    }
}

fn render_typedef(resolve: &Resolve, def: &wit_parser::TypeDef) -> String {
    match &def.kind {
        TypeDefKind::Record(record) => format!(
            "record{{{}}}",
            record
                .fields
                .iter()
                .map(|field| format!("{}:{}", field.name, render_type(resolve, &field.ty)))
                .collect::<Vec<_>>()
                .join(",")
        ),
        TypeDefKind::Enum(en) => format!(
            "enum{{{}}}",
            en.cases
                .iter()
                .map(|case| case.name.clone())
                .collect::<Vec<_>>()
                .join(",")
        ),
        TypeDefKind::Variant(variant) => format!(
            "variant{{{}}}",
            variant
                .cases
                .iter()
                .map(|case| match &case.ty {
                    Some(ty) => format!("{}({})", case.name, render_type(resolve, ty)),
                    None => case.name.clone(),
                })
                .collect::<Vec<_>>()
                .join(",")
        ),
        TypeDefKind::Flags(flags) => format!(
            "flags{{{}}}",
            flags
                .flags
                .iter()
                .map(|flag| flag.name.clone())
                .collect::<Vec<_>>()
                .join(",")
        ),
        TypeDefKind::Tuple(tuple) => format!(
            "tuple<{}>",
            tuple
                .types
                .iter()
                .map(|ty| render_type(resolve, ty))
                .collect::<Vec<_>>()
                .join(",")
        ),
        TypeDefKind::Option(inner) => format!("option<{}>", render_type(resolve, inner)),
        TypeDefKind::Result(result) => format!(
            "result<{},{}>",
            result
                .ok
                .map(|ty| render_type(resolve, &ty))
                .unwrap_or_else(|| "_".into()),
            result
                .err
                .map(|ty| render_type(resolve, &ty))
                .unwrap_or_else(|| "_".into())
        ),
        TypeDefKind::List(inner) => format!("list<{}>", render_type(resolve, inner)),
        TypeDefKind::Type(inner) => render_type(resolve, inner),
        other => other.as_str().to_string(),
    }
}

/// Every baseline item that vanished or changed shape. Additions are never
/// reported: see the module doc for why.
fn breaking_changes(baseline: &Snapshot, current: &Snapshot) -> Vec<String> {
    let mut breaks = Vec::new();

    for (name, shape) in &baseline.types {
        match current.types.get(name) {
            None => breaks.push(format!("type `{name}` was removed or renamed")),
            Some(current_shape) if current_shape != shape => breaks.push(format!(
                "type `{name}` changed shape: `{shape}` -> `{current_shape}`"
            )),
            _ => {}
        }
    }

    for (iface, functions) in &baseline.interfaces {
        match current.interfaces.get(iface) {
            None => breaks.push(format!("interface `{iface}` was removed or renamed")),
            Some(current_functions) => {
                for (function, signature) in functions {
                    match current_functions.get(function) {
                        None => breaks.push(format!(
                            "`{iface}.{function}` was removed or renamed"
                        )),
                        Some(current_signature) if current_signature != signature => {
                            breaks.push(format!(
                                "`{iface}.{function}` changed signature: `{signature}` -> `{current_signature}`"
                            ));
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    for (world, snapshot) in &baseline.worlds {
        match current.worlds.get(world) {
            None => breaks.push(format!("world `{world}` was removed or renamed")),
            Some(current_world) => {
                for export in &snapshot.exports {
                    if !current_world.exports.contains(export) {
                        breaks.push(format!("world `{world}` no longer exports `{export}`"));
                    }
                }
                for import in &snapshot.imports {
                    if !current_world.imports.contains(import) {
                        breaks.push(format!("world `{world}` no longer imports `{import}`"));
                    }
                }
            }
        }
    }

    breaks
}

fn resolve_current_wit() -> Resolve {
    let wit_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../wit");
    let mut resolve = Resolve::default();
    resolve
        .push_dir(&wit_dir)
        .expect("wit/orivo-plugin.wit must still parse");
    resolve
}

/// Not part of the suite: prints the current contract's snapshot so it can be
/// pasted into `wit-v1-baseline.json`. Only ever run by hand, and only when
/// `wit/orivo-plugin.wit` gains a deliberate, reviewed v2 — never to make this
/// file's own test pass.
///
/// `cargo test --manifest-path sdk/orivo-plugin-sdk/Cargo.toml \
///   --test wit_compatibility -- --ignored --nocapture dump_current_snapshot`
#[test]
#[ignore]
fn dump_current_snapshot_for_rebaselining() {
    let json = serde_json::to_string_pretty(&snapshot(&resolve_current_wit())).unwrap();
    println!("{json}");
}

#[test]
fn orivo_plugin_v1_has_no_breaking_change_against_its_frozen_baseline() {
    let current = snapshot(&resolve_current_wit());
    let baseline: Snapshot =
        serde_json::from_str(include_str!("wit-v1-baseline.json")).expect("baseline must parse");

    let breaks = breaking_changes(&baseline, &current);
    assert!(
        breaks.is_empty(),
        "orivo-plugin@1 changed in a way that breaks an existing component:\n  {}",
        breaks.join("\n  ")
    );
}

/// Proof the guard above actually guards something, without touching the real
/// WIT file to do it: a renamed export, a removed function and a narrowed
/// record are the exact three shapes the module doc calls breaking, and every
/// one of them is caught here. (An additive import is asserted absent from
/// the report for the same reason — see `admits_a_purely_additive_change`.)
#[test]
fn detects_a_renamed_export_a_removed_function_and_a_narrowed_record() {
    let mut baseline = Snapshot::default();
    baseline.worlds.insert(
        "runner-plugin".into(),
        WorldSnapshot {
            imports: BTreeSet::from(["host-files".into()]),
            exports: BTreeSet::from(["runner".into()]),
        },
    );
    baseline.interfaces.insert(
        "runner".into(),
        BTreeMap::from([
            ("prepare-launch".into(), "(profile-id:string)->result<types.launch-intent,_>".into()),
            ("validate-profile".into(), "(profile:types.runner-profile)->result<types.profile-validation,_>".into()),
        ]),
    );
    baseline.types.insert(
        "types.plugin-error".into(),
        "record{code:types.plugin-error-code,message:string,retryable:bool}".into(),
    );

    let mut current = baseline.clone();
    // Export renamed: the world no longer exports the name a component built
    // against v1 was compiled to provide.
    current.worlds.get_mut("runner-plugin").unwrap().exports =
        BTreeSet::from(["runner-v2".into()]);
    // Function removed from an interface that otherwise still exists.
    current
        .interfaces
        .get_mut("runner")
        .unwrap()
        .remove("validate-profile");
    // Record narrowed: a field an existing component reads is gone.
    current.types.insert(
        "types.plugin-error".into(),
        "record{code:types.plugin-error-code,message:string}".into(),
    );

    let breaks = breaking_changes(&baseline, &current);
    assert!(
        breaks
            .iter()
            .any(|b| b.contains("no longer exports `runner`")),
        "{breaks:#?}"
    );
    assert!(
        breaks
            .iter()
            .any(|b| b.contains("runner.validate-profile` was removed")),
        "{breaks:#?}"
    );
    assert!(
        breaks
            .iter()
            .any(|b| b.contains("types.plugin-error` changed shape")),
        "{breaks:#?}"
    );
    // Three real breaks, nothing else invented along the way.
    assert_eq!(breaks.len(), 3, "{breaks:#?}");
}

/// The mirror image of the test above: P1 added `host-files` and
/// `host-journal` as new imports to `runner-plugin` without a v2, precisely
/// because an addition cannot break a component that never uses it. The
/// comparator has to agree, or it would have rejected that exact change.
#[test]
fn admits_a_purely_additive_change() {
    let mut baseline = Snapshot::default();
    baseline.worlds.insert(
        "runner-plugin".into(),
        WorldSnapshot {
            imports: BTreeSet::new(),
            exports: BTreeSet::from(["plugin-core".into(), "runner".into()]),
        },
    );

    let mut current = baseline.clone();
    current.worlds.get_mut("runner-plugin").unwrap().imports =
        BTreeSet::from(["host-files".into(), "host-journal".into()]);

    assert!(breaking_changes(&baseline, &current).is_empty());
}
