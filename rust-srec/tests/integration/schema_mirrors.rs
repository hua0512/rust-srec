//! The OpenAPI and MCP schemas of many API types come from hand-written
//! mirrors in `srec-api/src/api/schemas.rs`, which keep `utoipa` and
//! `schemars` out of the lower crates. The compiler does not check that a
//! mirror still matches the type it copies, so this test parses both and
//! fails when what decides their JSON form drifts apart: field and variant
//! names, field types, and `serde` settings.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use syn::punctuated::Punctuated;
use syn::{Attribute, Expr, Fields, GenericArgument, Item, Lit, Meta, PathArguments, Token, Type};

/// Mirrors whose name more than one workspace type has, with the file of the
/// type each one copies, relative to `rust-srec/`.
const MIRRORED_FILES: &[(&str, &str)] = &[
    (
        "Priority",
        "crates/srec-core/src/domain/value_objects/priority.rs",
    ),
    ("EngineType", "crates/srec-core/src/domain/engine_config.rs"),
    ("JobPhase", "crates/srec-core/src/domain/processor.rs"),
];

#[derive(Debug, PartialEq)]
struct FieldShape {
    name: String,
    ty: String,
    serde: Vec<String>,
}

#[derive(Debug, PartialEq)]
struct VariantShape {
    name: String,
    serde: Vec<String>,
    fields: Vec<FieldShape>,
}

/// What decides a type's JSON form.
#[derive(Debug, PartialEq)]
enum Shape {
    Struct {
        serde: Vec<String>,
        fields: Vec<FieldShape>,
    },
    Enum {
        serde: Vec<String>,
        variants: Vec<VariantShape>,
    },
}

/// The `serde` settings of `attrs` that shape the JSON, sorted. A field is
/// optional whether it takes a default or is left out when empty, so both
/// read as `default`; converting through another type (`try_from`, `from`,
/// `into`) keeps the documented shape.
fn serde_attrs(attrs: &[Attribute]) -> Vec<String> {
    let mut settings = Vec::new();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("serde")) {
        let Meta::List(list) = &attr.meta else {
            continue;
        };
        let metas = list
            .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
            .unwrap_or_else(|e| panic!("parse serde({}): {e}", list.tokens));
        for meta in metas {
            let key = meta
                .path()
                .get_ident()
                .map_or_else(|| "?".to_owned(), ToString::to_string);
            let setting = match (&meta, key.as_str()) {
                (_, "default" | "skip_serializing_if") => "default".to_owned(),
                (_, "try_from" | "from" | "into") => continue,
                (Meta::Path(_), _) => key,
                (Meta::NameValue(pair), _) => {
                    let Expr::Lit(value) = &pair.value else {
                        panic!("serde({key}) with a non-literal value");
                    };
                    match &value.lit {
                        Lit::Str(value) => format!("{key} = {:?}", value.value()),
                        _ => panic!("serde({key}) with a non-string value"),
                    }
                }
                (Meta::List(nested), _) => format!("{key}({})", nested.tokens),
            };
            settings.push(setting);
        }
    }
    settings.sort();
    settings.dedup();
    settings
}

/// A type by the last segment of each path, so `srec_core::domain::Priority`
/// and a mirror's `Priority` compare equal.
fn type_name(ty: &Type) -> String {
    match ty {
        Type::Path(path) if path.qself.is_none() => {
            let Some(last) = path.path.segments.last() else {
                return "?".to_owned();
            };
            let mut name = last.ident.to_string();
            if let PathArguments::AngleBracketed(args) = &last.arguments {
                let args: Vec<String> = args
                    .args
                    .iter()
                    .map(|arg| match arg {
                        GenericArgument::Type(ty) => type_name(ty),
                        _ => "?".to_owned(),
                    })
                    .collect();
                name = format!("{name}<{}>", args.join(", "));
            }
            name
        }
        Type::Reference(reference) => format!("&{}", type_name(&reference.elem)),
        Type::Slice(slice) => format!("[{}]", type_name(&slice.elem)),
        Type::Array(array) => format!("[{}; _]", type_name(&array.elem)),
        Type::Tuple(tuple) => {
            let elems: Vec<String> = tuple.elems.iter().map(type_name).collect();
            format!("({})", elems.join(", "))
        }
        Type::Paren(inner) => type_name(&inner.elem),
        Type::Group(inner) => type_name(&inner.elem),
        _ => "?".to_owned(),
    }
}

fn field_shapes(fields: &Fields) -> Vec<FieldShape> {
    fields
        .iter()
        .enumerate()
        .map(|(index, field)| FieldShape {
            name: field
                .ident
                .as_ref()
                .map_or_else(|| index.to_string(), ToString::to_string),
            ty: type_name(&field.ty),
            serde: serde_attrs(&field.attrs),
        })
        // A skipped field never reaches JSON, so a mirror leaves it out.
        .filter(|field| !field.serde.iter().any(|attr| attr == "skip"))
        .collect()
}

fn shape(item: &Item) -> Option<(String, Shape)> {
    match item {
        Item::Struct(item) => Some((
            item.ident.to_string(),
            Shape::Struct {
                serde: serde_attrs(&item.attrs),
                fields: field_shapes(&item.fields),
            },
        )),
        Item::Enum(item) => Some((
            item.ident.to_string(),
            Shape::Enum {
                serde: serde_attrs(&item.attrs),
                variants: item
                    .variants
                    .iter()
                    .map(|variant| VariantShape {
                        name: variant.ident.to_string(),
                        serde: serde_attrs(&variant.attrs),
                        fields: field_shapes(&variant.fields),
                    })
                    .filter(|variant| !variant.serde.iter().any(|attr| attr == "skip"))
                    .collect(),
            },
        )),
        _ => None,
    }
}

fn is_cfg_test(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("cfg")
            && matches!(&attr.meta, Meta::List(list) if list.tokens.to_string() == "test")
    })
}

/// Every struct and enum in `items`, outside test modules.
fn collect_shapes(items: &[Item], file: &Path, out: &mut Vec<(String, PathBuf, Shape)>) {
    for item in items {
        if let Item::Mod(module) = item {
            if let Some((_, nested)) = &module.content
                && !is_cfg_test(&module.attrs)
            {
                collect_shapes(nested, file, out);
            }
        } else if let Some((name, shape)) = shape(item) {
            out.push((name, file.to_path_buf(), shape));
        }
    }
}

fn rust_files(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

fn parse(file: &Path) -> syn::File {
    let source =
        fs::read_to_string(file).unwrap_or_else(|e| panic!("read {}: {e}", file.display()));
    syn::parse_file(&source).unwrap_or_else(|e| panic!("parse {}: {e}", file.display()))
}

#[test]
fn schema_mirrors_match_the_types_they_copy() {
    let backend = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mirrors_file = backend.join("crates/srec-api/src/api/schemas.rs");
    let mut mirrors = Vec::new();
    collect_shapes(&parse(&mirrors_file).items, &mirrors_file, &mut mirrors);
    assert!(
        !mirrors.is_empty(),
        "no mirrors found in {}",
        mirrors_file.display()
    );

    let mut files = Vec::new();
    rust_files(&backend.join("src"), &mut files);
    rust_files(&backend.join("crates"), &mut files);
    let mut types: BTreeMap<String, Vec<(PathBuf, Shape)>> = BTreeMap::new();
    for file in files.iter().filter(|file| **file != mirrors_file) {
        let mut shapes = Vec::new();
        collect_shapes(&parse(file).items, file, &mut shapes);
        for (name, file, shape) in shapes {
            types.entry(name).or_default().push((file, shape));
        }
    }

    let mut problems = Vec::new();
    for (name, _, mirror) in &mirrors {
        let candidates = types.get(name).map(Vec::as_slice).unwrap_or_default();
        let copied = match candidates {
            [] => {
                problems.push(format!("{name}: no workspace type has this name"));
                continue;
            }
            [(_, only)] => only,
            several => {
                let Some((_, file)) = MIRRORED_FILES.iter().find(|(mirror, _)| mirror == name)
                else {
                    problems.push(format!(
                        "{name}: {} types have this name; add the copied one to MIRRORED_FILES",
                        several.len()
                    ));
                    continue;
                };
                let file = backend.join(file);
                match several.iter().find(|(path, _)| *path == file) {
                    Some((_, shape)) => shape,
                    None => {
                        problems.push(format!("{name}: no such type in {}", file.display()));
                        continue;
                    }
                }
            }
        };
        if copied != mirror {
            problems.push(format!(
                "{name}: the mirror differs from the type it copies\n  mirror: {mirror:?}\n  type:   {copied:?}"
            ));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
