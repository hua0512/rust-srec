use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use syn::ext::IdentExt;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::visit::{self, Visit};
use syn::{
    Attribute, Expr, Field, ImplItem, Item, ItemUse, Macro, Meta, Stmt, Token, TraitItem, UseTree,
    Variant,
};

/// Architectural components for every production module under `rust-srec/src/`.
///
/// The dependency relation in [`Component::allowed_deps`] is a strict acyclic
/// DAG mirroring the target five-layer workspace topology:
/// - Layer 0: `Core`, `Sqlite`
/// - Layer 1: `Engines`, `Processors`, `Notify`, `Logging`
/// - Layer 2: `Recorder`, `Pipeline`, `Metrics`
/// - Layer 3: `Api`
/// - Layer 4: `Services`, `AppEntry`
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Component {
    Core,
    Sqlite,
    Engines,
    Processors,
    Notify,
    Logging,
    Recorder,
    Pipeline,
    Metrics,
    Api,
    Services,
    AppEntry,
}

/// Classification of a file or `crate::...` path under `rust-srec/src/`.
///
/// `ErrorRoot` represents the application error (`src/error.rs`,
/// `crate::Error`, `crate::Result`), a re-export of the API crate's
/// `ServerError` that every root module returns; its own outward dependencies
/// are checked separately so the architectural [`Component`] graph stays
/// strictly acyclic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModuleSlot {
    Layer(Component),
    ErrorRoot,
}

impl Component {
    const ALL: &'static [Component] = &[
        Component::Core,
        Component::Sqlite,
        Component::Engines,
        Component::Processors,
        Component::Notify,
        Component::Logging,
        Component::Recorder,
        Component::Pipeline,
        Component::Metrics,
        Component::Api,
        Component::Services,
        Component::AppEntry,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Sqlite => "sqlite",
            Self::Engines => "engines",
            Self::Processors => "processors",
            Self::Notify => "notify",
            Self::Logging => "logging",
            Self::Recorder => "recorder",
            Self::Pipeline => "pipeline",
            Self::Metrics => "metrics",
            Self::Api => "api",
            Self::Services => "services",
            Self::AppEntry => "app_entry",
        }
    }

    /// Strict downward dependencies permitted for `self` (excluding self-edges).
    fn allowed_deps(self) -> &'static [Component] {
        match self {
            Self::Core => &[],
            Self::Sqlite => &[Self::Core],
            Self::Engines => &[Self::Core],
            Self::Processors => &[Self::Core],
            Self::Notify => &[Self::Core, Self::Sqlite],
            Self::Logging => &[Self::Core],
            Self::Recorder => &[Self::Core, Self::Sqlite, Self::Engines],
            Self::Pipeline => &[Self::Core, Self::Sqlite, Self::Processors],
            Self::Metrics => &[Self::Core, Self::Notify],
            Self::Api => &[
                Self::Core,
                Self::Sqlite,
                Self::Engines,
                Self::Processors,
                Self::Notify,
                Self::Logging,
                Self::Recorder,
                Self::Pipeline,
                Self::Metrics,
            ],
            Self::Services => &[
                Self::Core,
                Self::Sqlite,
                Self::Engines,
                Self::Processors,
                Self::Notify,
                Self::Logging,
                Self::Recorder,
                Self::Pipeline,
                Self::Metrics,
            ],
            Self::AppEntry => &[
                Self::Core,
                Self::Sqlite,
                Self::Engines,
                Self::Processors,
                Self::Notify,
                Self::Logging,
                Self::Recorder,
                Self::Pipeline,
                Self::Metrics,
                Self::Api,
                Self::Services,
            ],
        }
    }

    fn may_depend_on(self, target: Component) -> bool {
        self == target || self.allowed_deps().contains(&target)
    }
}

/// Map a module path relative to `rust-srec/src/` to its architectural slot.
///
/// Returns `None` for any unmapped top-level module so newly added files under
/// `rust-srec/src/` fail the DAG test instead of escaping verification.
fn slot_for_module_path(segments: &[&str]) -> Option<ModuleSlot> {
    match segments {
        [] => Some(ModuleSlot::Layer(Component::AppEntry)),
        ["lib" | "main" | "backend" | "server" | "panic_hook"] => {
            Some(ModuleSlot::Layer(Component::AppEntry))
        }
        ["bin", ..] => Some(ModuleSlot::Layer(Component::AppEntry)),
        ["error" | "Error" | "Result"] | ["error", ..] => Some(ModuleSlot::ErrorRoot),
        ["domain" | "utils", ..] => Some(ModuleSlot::Layer(Component::Core)),
        [
            "database",
            "repositories",
            "dag" | "job" | "staged_output" | "upload_record",
            ..,
        ] => Some(ModuleSlot::Layer(Component::Pipeline)),
        ["database", ..] => Some(ModuleSlot::Layer(Component::Sqlite)),
        ["downloader", "engine", ..] => Some(ModuleSlot::Layer(Component::Engines)),
        ["pipeline", "processors", ..] | ["baidupcs", ..] => {
            Some(ModuleSlot::Layer(Component::Processors))
        }
        ["notification" | "i18n", ..] => Some(ModuleSlot::Layer(Component::Notify)),
        ["logging", ..] => Some(ModuleSlot::Layer(Component::Logging)),
        ["metrics", ..] => Some(ModuleSlot::Layer(Component::Metrics)),
        [
            "recorder" | "config" | "credentials" | "proxies" | "monitor" | "downloader" | "danmu"
            | "session" | "streamer",
            ..,
        ] => Some(ModuleSlot::Layer(Component::Recorder)),
        ["pipeline", ..] => Some(ModuleSlot::Layer(Component::Pipeline)),
        ["api" | "mcp", ..] => Some(ModuleSlot::Layer(Component::Api)),
        ["services" | "runtime", ..] => Some(ModuleSlot::Layer(Component::Services)),
        _ => None,
    }
}

fn rust_files(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in
        fs::read_dir(directory).unwrap_or_else(|e| panic!("read {}: {e}", directory.display()))
    {
        let path = entry.expect("read directory entry").path();
        if path.is_dir() {
            rust_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

fn is_named_test_source(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name == "tests.rs"
        || name.ends_with("_tests.rs")
        || name == "test_support.rs"
        || name == "test_utils.rs"
    {
        return true;
    }
    path.components()
        .any(|component| component.as_os_str() == "tests")
}

/// Convert a file under `src_root` into its Rust module path segments.
fn file_module_segments(src_root: &Path, file: &Path) -> Vec<String> {
    let rel = file.strip_prefix(src_root).expect("file under src root");
    let mut segments: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if let Some(last) = segments.last_mut()
        && let Some(stem) = last.strip_suffix(".rs")
    {
        *last = stem.to_owned();
    }
    if matches!(segments.as_slice(), [root] if root == "lib" || root == "main") {
        segments.clear();
    }
    segments
}

/// Parsed subset of `#[cfg(...)]` predicates needed to distinguish test-only
/// items from production code.
enum CfgPred {
    Test,
    All(Vec<CfgPred>),
    Any(Vec<CfgPred>),
    Not,
    Other,
}

impl CfgPred {
    fn requires_test(&self) -> bool {
        match self {
            Self::Test => true,
            Self::All(preds) => preds.iter().any(Self::requires_test),
            Self::Any(preds) => !preds.is_empty() && preds.iter().all(Self::requires_test),
            Self::Not | Self::Other => false,
        }
    }
}

impl Parse for CfgPred {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let ident = input.call(syn::Ident::parse_any)?;
        let name = ident.to_string();
        if input.peek(syn::token::Paren) {
            let content;
            syn::parenthesized!(content in input);
            let nested: Punctuated<CfgPred, Token![,]> =
                content.parse_terminated(CfgPred::parse, Token![,])?;
            let items: Vec<CfgPred> = nested.into_iter().collect();
            return Ok(match name.as_str() {
                "all" => Self::All(items),
                "any" => Self::Any(items),
                "not" => Self::Not,
                _ => Self::Other,
            });
        }
        if input.peek(Token![=]) {
            input.parse::<Token![=]>()?;
            let _: syn::Lit = input.parse()?;
            return Ok(Self::Other);
        }
        Ok(if name == "test" {
            Self::Test
        } else {
            Self::Other
        })
    }
}

fn attr_is_cfg_test(attr: &Attribute) -> bool {
    if !attr.path().is_ident("cfg") {
        return false;
    }
    let Meta::List(list) = &attr.meta else {
        return false;
    };
    list.parse_args::<CfgPred>()
        .map(|pred| pred.requires_test())
        .unwrap_or(false)
}

fn has_cfg_test(attrs: &[Attribute]) -> bool {
    attrs.iter().any(attr_is_cfg_test)
}

/// Discover every out-of-line module file declared under `#[cfg(test)]` (for
/// example `#[cfg(test)] mod coverage;` in `src/api/openapi.rs`).
fn collect_cfg_test_submodule_paths(src_root: &Path, files: &[PathBuf]) -> BTreeSet<PathBuf> {
    fn visit_items(
        items: &[Item],
        dir: &Path,
        inside_cfg_test: bool,
        test_paths: &mut BTreeSet<PathBuf>,
    ) {
        for item in items {
            let Item::Mod(item_mod) = item else {
                continue;
            };
            let is_test = inside_cfg_test || has_cfg_test(&item_mod.attrs);
            if let Some((_, nested)) = &item_mod.content {
                let next_dir = dir.join(item_mod.ident.to_string());
                visit_items(nested, &next_dir, is_test, test_paths);
            } else if is_test {
                let name = item_mod.ident.to_string();
                test_paths.insert(dir.join(format!("{name}.rs")));
                test_paths.insert(dir.join(&name));
            }
        }
    }

    let mut test_paths = BTreeSet::new();
    for file in files {
        let source =
            fs::read_to_string(file).unwrap_or_else(|e| panic!("read {}: {e}", file.display()));
        let syntax =
            syn::parse_file(&source).unwrap_or_else(|e| panic!("parse {}: {e}", file.display()));
        let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let child_dir = if stem == "lib" || stem == "main" {
            file.parent().unwrap_or(src_root).to_path_buf()
        } else {
            file.with_extension("")
        };
        visit_items(&syntax.items, &child_dir, false, &mut test_paths);
    }
    test_paths
}

fn is_cfg_test_submodule(file: &Path, test_paths: &BTreeSet<PathBuf>) -> bool {
    test_paths.contains(file)
        || file
            .ancestors()
            .skip(1)
            .any(|ancestor| test_paths.contains(ancestor))
}

#[derive(Default)]
struct ExtractedRefs {
    /// Resolved `crate::...` internal module paths (without the leading `crate`).
    internal_paths: BTreeSet<Vec<String>>,
    /// External crate root identifiers (from `use`, type/expr/macro paths, and attributes).
    external_roots: BTreeSet<String>,
    /// Unqualified macro or derive identifiers (e.g. `ToSchema`, `t_str`).
    bare_markers: BTreeSet<String>,
    /// Paths starting with `rust_srec` (checked by the desktop facade guard).
    rust_srec_paths: BTreeSet<Vec<String>>,
    /// Whether `#[macro_export]` appears in production code.
    has_macro_export: bool,
}

type ScannedPath = (bool, Vec<String>, bool);

/// Walk arbitrary token streams (macro invocations, attribute arguments) and
/// collect maximal `::`-separated identifier paths. Because `syn` has already
/// tokenized comments and string literals, log filter strings and doc comments
/// never trigger false positives.
fn scan_parse_stream(input: ParseStream<'_>, out: &mut Vec<ScannedPath>) -> syn::Result<()> {
    while !input.is_empty() {
        if input.peek(syn::token::Paren) {
            let content;
            syn::parenthesized!(content in input);
            scan_parse_stream(&content, out)?;
        } else if input.peek(syn::token::Bracket) {
            let content;
            syn::bracketed!(content in input);
            scan_parse_stream(&content, out)?;
        } else if input.peek(syn::token::Brace) {
            let content;
            syn::braced!(content in input);
            scan_parse_stream(&content, out)?;
        } else if (input.peek(Token![::]) && input.peek2(syn::Ident::peek_any))
            || input.peek(syn::Ident::peek_any)
        {
            let leading_colon = if input.peek(Token![::]) {
                input.parse::<Token![::]>()?;
                true
            } else {
                false
            };
            let mut segments = vec![input.call(syn::Ident::parse_any)?.to_string()];
            while input.peek(Token![::]) && input.peek2(syn::Ident::peek_any) {
                input.parse::<Token![::]>()?;
                segments.push(input.call(syn::Ident::parse_any)?.to_string());
            }
            let is_macro = input.peek(Token![!]) && !input.peek(Token![!=]);
            if is_macro {
                input.parse::<Token![!]>()?;
            }
            out.push((leading_colon, segments, is_macro));
        } else {
            input.step(|cursor| {
                cursor
                    .token_tree()
                    .map(|(_, next)| ((), next))
                    .ok_or_else(|| cursor.error("unexpected end of token stream"))
            })?;
        }
    }
    Ok(())
}

struct RefCollector {
    module_stack: Vec<String>,
    skip_cfg_test: bool,
    refs: ExtractedRefs,
}

impl RefCollector {
    fn new(module_segments: Vec<String>, skip_cfg_test: bool) -> Self {
        Self {
            module_stack: module_segments,
            skip_cfg_test,
            refs: ExtractedRefs::default(),
        }
    }

    fn record_scanned(&mut self, scanned: Vec<ScannedPath>) {
        for (leading, segments, is_macro) in scanned {
            self.record_path_segments(leading, &segments, is_macro);
        }
    }

    fn record_path_segments(&mut self, leading_colon: bool, segments: &[String], is_macro: bool) {
        let Some(first) = segments.first() else {
            return;
        };

        if first == "rust_srec" {
            self.refs.rust_srec_paths.insert(segments.to_vec());
            return;
        }

        if !leading_colon && first == "crate" {
            if segments.len() >= 2 {
                self.refs.internal_paths.insert(segments[1..].to_vec());
            }
            return;
        }

        if !leading_colon && (first == "self" || first == "super") {
            let mut resolved = self.module_stack.clone();
            let mut idx = 0;
            while idx < segments.len() {
                match segments[idx].as_str() {
                    "self" => idx += 1,
                    "super" => {
                        resolved.pop();
                        idx += 1;
                    }
                    _ => break,
                }
            }
            resolved.extend_from_slice(&segments[idx..]);
            if !resolved.is_empty() {
                self.refs.internal_paths.insert(resolved);
            }
            return;
        }

        if segments.len() >= 2 || leading_colon {
            if is_crate_ident(first) {
                self.refs.external_roots.insert(first.clone());
            }
            if is_macro && let Some(last) = segments.last() {
                self.refs.bare_markers.insert(last.clone());
            }
        } else if is_macro {
            self.refs.bare_markers.insert(first.clone());
        }
    }

    fn record_use_tree(&mut self, leading_colon: bool, tree: &UseTree, prefix: &mut Vec<String>) {
        match tree {
            UseTree::Path(path) => {
                prefix.push(path.ident.to_string());
                self.record_use_tree(leading_colon, &path.tree, prefix);
                prefix.pop();
            }
            UseTree::Name(name) => {
                let ident = name.ident.to_string();
                if ident == "self" {
                    self.record_path_segments(leading_colon, prefix, false);
                } else {
                    prefix.push(ident.clone());
                    self.record_path_segments(leading_colon, prefix, false);
                    if prefix.len() == 1 {
                        if is_crate_ident(&ident) {
                            self.refs.external_roots.insert(ident.clone());
                        }
                        self.refs.bare_markers.insert(ident);
                    }
                    prefix.pop();
                }
            }
            UseTree::Rename(rename) => {
                let ident = rename.ident.to_string();
                if ident == "self" {
                    self.record_path_segments(leading_colon, prefix, false);
                } else {
                    prefix.push(ident.clone());
                    self.record_path_segments(leading_colon, prefix, false);
                    if prefix.len() == 1 && is_crate_ident(&ident) {
                        self.refs.external_roots.insert(ident);
                    }
                    prefix.pop();
                }
            }
            UseTree::Glob(_) => {
                self.record_path_segments(leading_colon, prefix, false);
            }
            UseTree::Group(group) => {
                for item in &group.items {
                    self.record_use_tree(leading_colon, item, prefix);
                }
            }
        }
    }
    fn record_meta(&mut self, meta: &Meta) {
        let path = meta.path();
        if path.is_ident("doc") || path.is_ident("cfg") {
            return;
        }
        if path.is_ident("macro_export") {
            self.refs.has_macro_export = true;
            return;
        }
        if path.is_ident("derive") {
            if let Meta::List(list) = meta
                && let Ok(derives) =
                    list.parse_args_with(Punctuated::<syn::Path, Token![,]>::parse_terminated)
            {
                for derive_path in derives {
                    let segments: Vec<String> = derive_path
                        .segments
                        .iter()
                        .map(|s| s.ident.to_string())
                        .collect();
                    if segments.len() == 1 {
                        self.refs.bare_markers.insert(segments[0].clone());
                    } else {
                        self.record_path_segments(
                            derive_path.leading_colon.is_some(),
                            &segments,
                            false,
                        );
                    }
                }
            }
            return;
        }
        if path.is_ident("cfg_attr") {
            if let Meta::List(list) = meta {
                let skip_cfg_test = self.skip_cfg_test;
                let mut nested_metas = Vec::new();
                let _ = list.parse_args_with(|input: ParseStream<'_>| {
                    let pred: CfgPred = input.parse()?;
                    let _: Token![,] = input.parse()?;
                    if skip_cfg_test && pred.requires_test() {
                        while !input.is_empty() {
                            input.step(|cursor| {
                                cursor
                                    .token_tree()
                                    .map(|(_, next)| ((), next))
                                    .ok_or_else(|| cursor.error("eof"))
                            })?;
                        }
                        return Ok(());
                    }
                    let metas = Punctuated::<Meta, Token![,]>::parse_terminated(input)?;
                    nested_metas.extend(metas);
                    Ok(())
                });
                for nested in &nested_metas {
                    self.record_meta(nested);
                }
            }
            return;
        }

        let segments: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        if segments.len() == 1 {
            if is_crate_ident(&segments[0]) {
                // Captures helper attributes such as `#[sqlx(...)]`, `#[schemars(...)]`,
                // `#[prost(...)]`, and `#[utoipa(...)]`.
                self.refs.external_roots.insert(segments[0].clone());
            }
        } else {
            self.record_path_segments(path.leading_colon.is_some(), &segments, false);
        }

        if let Meta::List(list) = meta {
            let mut scanned = Vec::new();
            let _ = list
                .parse_args_with(|input: ParseStream<'_>| scan_parse_stream(input, &mut scanned));
            self.record_scanned(scanned);
        }
    }
}

fn is_crate_ident(ident: &str) -> bool {
    !matches!(ident, "std" | "core" | "alloc" | "Self")
        && ident
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
}

fn item_attrs(item: &Item) -> &[Attribute] {
    match item {
        Item::Const(i) => &i.attrs,
        Item::Enum(i) => &i.attrs,
        Item::ExternCrate(i) => &i.attrs,
        Item::Fn(i) => &i.attrs,
        Item::ForeignMod(i) => &i.attrs,
        Item::Impl(i) => &i.attrs,
        Item::Macro(i) => &i.attrs,
        Item::Mod(i) => &i.attrs,
        Item::Static(i) => &i.attrs,
        Item::Struct(i) => &i.attrs,
        Item::Trait(i) => &i.attrs,
        Item::TraitAlias(i) => &i.attrs,
        Item::Type(i) => &i.attrs,
        Item::Union(i) => &i.attrs,
        Item::Use(i) => &i.attrs,
        _ => &[],
    }
}

fn impl_item_attrs(item: &ImplItem) -> &[Attribute] {
    match item {
        ImplItem::Const(i) => &i.attrs,
        ImplItem::Fn(i) => &i.attrs,
        ImplItem::Type(i) => &i.attrs,
        ImplItem::Macro(i) => &i.attrs,
        _ => &[],
    }
}

fn trait_item_attrs(item: &TraitItem) -> &[Attribute] {
    match item {
        TraitItem::Const(i) => &i.attrs,
        TraitItem::Fn(i) => &i.attrs,
        TraitItem::Type(i) => &i.attrs,
        TraitItem::Macro(i) => &i.attrs,
        _ => &[],
    }
}

fn expr_attrs(expr: &Expr) -> &[Attribute] {
    match expr {
        Expr::Array(e) => &e.attrs,
        Expr::Assign(e) => &e.attrs,
        Expr::Async(e) => &e.attrs,
        Expr::Await(e) => &e.attrs,
        Expr::Binary(e) => &e.attrs,
        Expr::Block(e) => &e.attrs,
        Expr::Break(e) => &e.attrs,
        Expr::Call(e) => &e.attrs,
        Expr::Cast(e) => &e.attrs,
        Expr::Closure(e) => &e.attrs,
        Expr::Const(e) => &e.attrs,
        Expr::Continue(e) => &e.attrs,
        Expr::Field(e) => &e.attrs,
        Expr::ForLoop(e) => &e.attrs,
        Expr::Group(e) => &e.attrs,
        Expr::If(e) => &e.attrs,
        Expr::Index(e) => &e.attrs,
        Expr::Infer(e) => &e.attrs,
        Expr::Let(e) => &e.attrs,
        Expr::Lit(e) => &e.attrs,
        Expr::Loop(e) => &e.attrs,
        Expr::Macro(e) => &e.attrs,
        Expr::Match(e) => &e.attrs,
        Expr::MethodCall(e) => &e.attrs,
        Expr::Paren(e) => &e.attrs,
        Expr::Path(e) => &e.attrs,
        Expr::Range(e) => &e.attrs,
        Expr::RawAddr(e) => &e.attrs,
        Expr::Reference(e) => &e.attrs,
        Expr::Repeat(e) => &e.attrs,
        Expr::Return(e) => &e.attrs,
        Expr::Struct(e) => &e.attrs,
        Expr::Try(e) => &e.attrs,
        Expr::TryBlock(e) => &e.attrs,
        Expr::Tuple(e) => &e.attrs,
        Expr::Unary(e) => &e.attrs,
        Expr::Unsafe(e) => &e.attrs,
        Expr::While(e) => &e.attrs,
        Expr::Yield(e) => &e.attrs,
        _ => &[],
    }
}

impl<'ast> Visit<'ast> for RefCollector {
    fn visit_item(&mut self, item: &'ast Item) {
        if self.skip_cfg_test && has_cfg_test(item_attrs(item)) {
            return;
        }
        match item {
            Item::Mod(item_mod) => {
                for attr in &item_mod.attrs {
                    self.visit_attribute(attr);
                }
                if let Some((_, items)) = &item_mod.content {
                    self.module_stack.push(item_mod.ident.to_string());
                    for nested in items {
                        self.visit_item(nested);
                    }
                    self.module_stack.pop();
                }
            }
            Item::ExternCrate(ext) => {
                self.refs.external_roots.insert(ext.ident.to_string());
                visit::visit_item_extern_crate(self, ext);
            }
            _ => visit::visit_item(self, item),
        }
    }

    fn visit_impl_item(&mut self, item: &'ast ImplItem) {
        if self.skip_cfg_test && has_cfg_test(impl_item_attrs(item)) {
            return;
        }
        visit::visit_impl_item(self, item);
    }

    fn visit_trait_item(&mut self, item: &'ast TraitItem) {
        if self.skip_cfg_test && has_cfg_test(trait_item_attrs(item)) {
            return;
        }
        visit::visit_trait_item(self, item);
    }

    fn visit_field(&mut self, field: &'ast Field) {
        if self.skip_cfg_test && has_cfg_test(&field.attrs) {
            return;
        }
        visit::visit_field(self, field);
    }

    fn visit_variant(&mut self, variant: &'ast Variant) {
        if self.skip_cfg_test && has_cfg_test(&variant.attrs) {
            return;
        }
        visit::visit_variant(self, variant);
    }

    fn visit_stmt(&mut self, stmt: &'ast Stmt) {
        let attrs = match stmt {
            Stmt::Local(local) => local.attrs.as_slice(),
            Stmt::Item(item) => item_attrs(item),
            Stmt::Expr(expr, _) => expr_attrs(expr),
            Stmt::Macro(mac) => mac.attrs.as_slice(),
        };
        if self.skip_cfg_test && has_cfg_test(attrs) {
            return;
        }
        visit::visit_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'ast Expr) {
        if self.skip_cfg_test && has_cfg_test(expr_attrs(expr)) {
            return;
        }
        visit::visit_expr(self, expr);
    }

    fn visit_item_use(&mut self, item_use: &'ast ItemUse) {
        let mut prefix = Vec::new();
        self.record_use_tree(
            item_use.leading_colon.is_some(),
            &item_use.tree,
            &mut prefix,
        );
        for attr in &item_use.attrs {
            self.visit_attribute(attr);
        }
    }

    fn visit_path(&mut self, path: &'ast syn::Path) {
        let segments: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        self.record_path_segments(path.leading_colon.is_some(), &segments, false);
        visit::visit_path(self, path);
    }

    fn visit_macro(&mut self, mac: &'ast Macro) {
        let segments: Vec<String> = mac
            .path
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect();
        self.record_path_segments(mac.path.leading_colon.is_some(), &segments, true);
        let mut scanned = Vec::new();
        let _ =
            mac.parse_body_with(|input: ParseStream<'_>| scan_parse_stream(input, &mut scanned));
        self.record_scanned(scanned);
        visit::visit_macro(self, mac);
    }

    fn visit_attribute(&mut self, attr: &'ast Attribute) {
        self.record_meta(&attr.meta);
    }
}

fn extract_file_refs(src_root: &Path, file: &Path, skip_cfg_test: bool) -> ExtractedRefs {
    let source =
        fs::read_to_string(file).unwrap_or_else(|e| panic!("read {}: {e}", file.display()));
    let syntax =
        syn::parse_file(&source).unwrap_or_else(|e| panic!("parse {}: {e}", file.display()));
    let mut collector = RefCollector::new(file_module_segments(src_root, file), skip_cfg_test);
    collector.visit_file(&syntax);
    collector.refs
}

/// Source classification for every analyzed file across `rust-srec/src/` and
/// extracted internal crates under `rust-srec/crates/srec-*`.
struct AnalyzedFile {
    display_path: String,
    slot: ModuleSlot,
    /// True when `crate::...` paths in this file resolve against
    /// `slot_for_module_path` (root `rust-srec/src/` and `crates/srec-api/src/`,
    /// where `logging` and `metrics` sit beside `api`/`mcp`).
    check_internal_paths: bool,
    /// True when the file lives in `rust-srec/src/`.
    is_root_crate: bool,
    refs: ExtractedRefs,
}

fn component_for_external_crate(ident: &str) -> Option<Component> {
    match ident {
        "srec_core" => Some(Component::Core),
        "srec_sqlite" => Some(Component::Sqlite),
        "srec_engines" => Some(Component::Engines),
        "srec_processors" => Some(Component::Processors),
        "srec_notify" => Some(Component::Notify),
        "srec_pipeline" => Some(Component::Pipeline),
        "srec_recorder" => Some(Component::Recorder),
        "srec_api" => Some(Component::Api),
        "rust_srec" => Some(Component::AppEntry),
        _ => None,
    }
}

fn analyze_extracted_crate(
    crate_dir: &Path,
    display_prefix: &str,
    component: Component,
    include_unit_tests: bool,
    out: &mut Vec<AnalyzedFile>,
) {
    let src_root = crate_dir.join("src");
    let mut files = Vec::new();
    rust_files(&src_root, &mut files);
    files.sort();
    assert!(
        !files.is_empty(),
        "expected Rust source files under {}",
        src_root.display()
    );

    let cfg_test_submodules = collect_cfg_test_submodule_paths(&src_root, &files);
    for file in files {
        if !include_unit_tests
            && (is_named_test_source(&file) || is_cfg_test_submodule(&file, &cfg_test_submodules))
        {
            continue;
        }
        let rel = file
            .strip_prefix(&src_root)
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");
        let refs = extract_file_refs(&src_root, &file, !include_unit_tests);
        out.push(AnalyzedFile {
            display_path: format!("{display_prefix}/{rel}"),
            slot: ModuleSlot::Layer(component),
            check_internal_paths: false,
            is_root_crate: false,
            refs,
        });
    }
}

fn analyze_srec_api_crate(crate_dir: &Path, out: &mut Vec<AnalyzedFile>) {
    let src_root = crate_dir.join("src");
    let mut files = Vec::new();
    rust_files(&src_root, &mut files);
    files.sort();
    assert!(
        !files.is_empty(),
        "expected Rust source files under {}",
        src_root.display()
    );

    let cfg_test_submodules = collect_cfg_test_submodule_paths(&src_root, &files);
    for file in files {
        if is_named_test_source(&file) || is_cfg_test_submodule(&file, &cfg_test_submodules) {
            continue;
        }
        let segs = file_module_segments(&src_root, &file);
        let seg_refs: Vec<&str> = segs.iter().map(String::as_str).collect();
        let component = match seg_refs.as_slice() {
            ["logging", ..] => Component::Logging,
            ["metrics", ..] => Component::Metrics,
            _ => Component::Api,
        };
        let rel = file
            .strip_prefix(&src_root)
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");
        let refs = extract_file_refs(&src_root, &file, true);
        out.push(AnalyzedFile {
            display_path: format!("crates/srec-api/src/{rel}"),
            slot: ModuleSlot::Layer(component),
            check_internal_paths: true,
            is_root_crate: false,
            refs,
        });
    }
}

fn production_source_files() -> Vec<AnalyzedFile> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src_root = manifest_dir.join("src");
    let mut files = Vec::new();
    rust_files(&src_root, &mut files);
    files.sort();

    let cfg_test_submodules = collect_cfg_test_submodule_paths(&src_root, &files);

    let mut unmapped = Vec::new();
    let mut analyzed = Vec::new();

    // Scan extracted Layer 0, Layer 1, Layer 2, and Layer 3 workspace crates directly.
    // `srec-core` includes its unit tests so domain and utility tests stay
    // storage-free as well.
    analyze_extracted_crate(
        &manifest_dir.join("crates/srec-core"),
        "crates/srec-core/src",
        Component::Core,
        true,
        &mut analyzed,
    );
    analyze_extracted_crate(
        &manifest_dir.join("crates/srec-sqlite"),
        "crates/srec-sqlite/src",
        Component::Sqlite,
        false,
        &mut analyzed,
    );
    analyze_extracted_crate(
        &manifest_dir.join("crates/srec-engines"),
        "crates/srec-engines/src",
        Component::Engines,
        false,
        &mut analyzed,
    );
    analyze_extracted_crate(
        &manifest_dir.join("crates/srec-processors"),
        "crates/srec-processors/src",
        Component::Processors,
        false,
        &mut analyzed,
    );
    analyze_extracted_crate(
        &manifest_dir.join("crates/srec-notify"),
        "crates/srec-notify/src",
        Component::Notify,
        false,
        &mut analyzed,
    );
    analyze_extracted_crate(
        &manifest_dir.join("crates/srec-pipeline"),
        "crates/srec-pipeline/src",
        Component::Pipeline,
        false,
        &mut analyzed,
    );
    analyze_extracted_crate(
        &manifest_dir.join("crates/srec-recorder"),
        "crates/srec-recorder/src",
        Component::Recorder,
        false,
        &mut analyzed,
    );
    analyze_srec_api_crate(&manifest_dir.join("crates/srec-api"), &mut analyzed);

    for file in files {
        let segs = file_module_segments(&src_root, &file);
        if is_named_test_source(&file) || is_cfg_test_submodule(&file, &cfg_test_submodules) {
            continue;
        }
        let seg_refs: Vec<&str> = segs.iter().map(String::as_str).collect();
        let Some(slot) = slot_for_module_path(&seg_refs) else {
            unmapped.push(
                file.strip_prefix(&src_root)
                    .unwrap_or(&file)
                    .display()
                    .to_string(),
            );
            continue;
        };
        let rel = file
            .strip_prefix(&src_root)
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");
        let refs = extract_file_refs(&src_root, &file, true);
        analyzed.push(AnalyzedFile {
            display_path: rel,
            slot,
            check_internal_paths: true,
            is_root_crate: true,
            refs,
        });
    }

    assert!(
        unmapped.is_empty(),
        "every production file under rust-srec/src/ must be mapped into the layer DAG:\n{}",
        unmapped.join("\n")
    );

    analyzed
}

fn check_layer_edge(
    display_path: &str,
    source_slot: ModuleSlot,
    target_slot: ModuleSlot,
    via: &str,
    violations: &mut Vec<String>,
) {
    match (source_slot, target_slot) {
        // Every layer returns `crate::Error` / `crate::Result` while hosted in `rust-srec`.
        (_, ModuleSlot::ErrorRoot) => {}
        // `src/error.rs` may only reference Layer 0 types (`Core`, `Sqlite`) and
        // the API crate, whose `ServerError` wraps every lower crate's error.
        (ModuleSlot::ErrorRoot, ModuleSlot::Layer(target)) => {
            if !matches!(target, Component::Core | Component::Sqlite | Component::Api) {
                violations.push(format!(
                    "{display_path}: error_root -> {} via {via}",
                    target.name()
                ));
            }
        }
        (ModuleSlot::Layer(src), ModuleSlot::Layer(dst)) => {
            if !src.may_depend_on(dst) {
                violations.push(format!(
                    "{display_path}: {} -> {} via {via}",
                    src.name(),
                    dst.name()
                ));
            }
        }
    }
}

#[test]
fn production_modules_respect_acyclic_layered_dag() {
    // Verify via Kahn's topological sort that Component::allowed_deps is strictly
    // acyclic so mutual dependencies cannot be added to the guard table.
    let mut in_degree: BTreeMap<Component, usize> =
        Component::ALL.iter().map(|&c| (c, 0)).collect();
    for &src in Component::ALL {
        for &dst in src.allowed_deps() {
            assert_ne!(src, dst, "self-edge must not appear in allowed_deps");
            *in_degree.entry(dst).or_default() += 1;
        }
    }
    let mut queue: VecDeque<Component> = in_degree
        .iter()
        .filter_map(|(&c, &deg)| (deg == 0).then_some(c))
        .collect();
    let mut visited = 0usize;
    while let Some(node) = queue.pop_front() {
        visited += 1;
        for &dst in node.allowed_deps() {
            let deg = in_degree.get_mut(&dst).expect("known component");
            *deg -= 1;
            if *deg == 0 {
                queue.push_back(dst);
            }
        }
    }
    assert_eq!(
        visited,
        Component::ALL.len(),
        "Component::allowed_deps must be acyclic"
    );

    let analyzed = production_source_files();
    assert!(
        !analyzed.is_empty(),
        "expected production Rust files under rust-srec/src/ and rust-srec/crates/"
    );

    let mut violations = Vec::new();
    for entry in &analyzed {
        if entry.check_internal_paths {
            for internal_path in &entry.refs.internal_paths {
                let segs: Vec<&str> = internal_path.iter().map(String::as_str).collect();
                let Some(target_slot) = slot_for_module_path(&segs) else {
                    continue;
                };
                check_layer_edge(
                    &entry.display_path,
                    entry.slot,
                    target_slot,
                    &format!("crate::{}", internal_path.join("::")),
                    &mut violations,
                );
            }
        }
        for ext in &entry.refs.external_roots {
            if let Some(target_component) = component_for_external_crate(ext) {
                check_layer_edge(
                    &entry.display_path,
                    entry.slot,
                    ModuleSlot::Layer(target_component),
                    ext,
                    &mut violations,
                );
            }
        }
        if !entry.is_root_crate && !entry.refs.rust_srec_paths.is_empty() {
            for path in &entry.refs.rust_srec_paths {
                check_layer_edge(
                    &entry.display_path,
                    entry.slot,
                    ModuleSlot::Layer(Component::AppEntry),
                    &path.join("::"),
                    &mut violations,
                );
            }
        }
    }

    assert!(
        violations.is_empty(),
        "production modules violated the acyclic layered DAG:\n{}",
        violations.join("\n")
    );
}

#[test]
fn heavy_external_dependencies_stay_within_target_crates() {
    struct ExternalRule {
        crates: &'static [&'static str],
        bare_markers: &'static [&'static str],
        allowed_slots: &'static [ModuleSlot],
        allowed_files: &'static [&'static str],
        rationale: &'static str,
    }

    const RULES: &[ExternalRule] = &[
        ExternalRule {
            crates: &["mesio", "mesio_engine", "flv_fix", "hls_fix"],
            bare_markers: &[],
            allowed_slots: &[ModuleSlot::Layer(Component::Engines)],
            allowed_files: &[],
            rationale: "media download engines belong exclusively to srec-engines",
        },
        ExternalRule {
            crates: &["zip", "tar"],
            bare_markers: &[],
            allowed_slots: &[ModuleSlot::Layer(Component::Processors)],
            allowed_files: &[],
            rationale: "archive compression/extraction belongs exclusively to srec-processors",
        },
        ExternalRule {
            crates: &["lettre", "aes_gcm", "hkdf"],
            bare_markers: &[],
            allowed_slots: &[ModuleSlot::Layer(Component::Notify)],
            allowed_files: &[],
            rationale: "SMTP and Web Push encryption belong exclusively to srec-notify",
        },
        ExternalRule {
            crates: &["p256"],
            bare_markers: &[],
            allowed_slots: &[ModuleSlot::Layer(Component::Notify)],
            allowed_files: &[],
            rationale: "VAPID P-256 keys belong exclusively to srec-notify",
        },
        ExternalRule {
            crates: &["rust_i18n"],
            bare_markers: &["t_str", "t_str_in"],
            allowed_slots: &[ModuleSlot::Layer(Component::Notify)],
            allowed_files: &[],
            rationale: "backend i18n belongs exclusively to srec-notify",
        },
        ExternalRule {
            crates: &[
                "axum",
                "utoipa",
                "schemars",
                "rmcp",
                "jsonwebtoken",
                "argon2",
                "prost",
            ],
            bare_markers: &["ToSchema", "IntoParams", "JsonSchema"],
            allowed_slots: &[ModuleSlot::Layer(Component::Api)],
            allowed_files: &[],
            rationale: "HTTP, OpenAPI, MCP, auth crypto, and Protobuf belong exclusively to srec-api",
        },
        ExternalRule {
            crates: &["platforms_parser"],
            bare_markers: &[],
            allowed_slots: &[
                ModuleSlot::ErrorRoot,
                ModuleSlot::Layer(Component::Recorder),
                ModuleSlot::Layer(Component::Api),
                ModuleSlot::Layer(Component::Services),
                ModuleSlot::Layer(Component::AppEntry),
            ],
            allowed_files: &[],
            rationale: "platforms_parser is forbidden in core, sqlite, engines, processors, notify, pipeline, logging, and metrics",
        },
        ExternalRule {
            crates: &["sqlx"],
            bare_markers: &[],
            allowed_slots: &[
                ModuleSlot::ErrorRoot,
                ModuleSlot::Layer(Component::Sqlite),
                ModuleSlot::Layer(Component::Notify),
                ModuleSlot::Layer(Component::Pipeline),
                ModuleSlot::Layer(Component::Recorder),
                ModuleSlot::Layer(Component::Api),
                ModuleSlot::Layer(Component::Services),
                ModuleSlot::Layer(Component::AppEntry),
            ],
            allowed_files: &[],
            rationale: "sqlx is forbidden in core (domain/utils), engines, processors, logging, and metrics",
        },
    ];

    let analyzed = production_source_files();
    let mut violations = Vec::new();

    for entry in &analyzed {
        let rel = entry.display_path.as_str();

        if (rel == "i18n.rs" || rel == "crates/srec-notify/src/i18n.rs")
            && entry.refs.has_macro_export
        {
            violations.push(format!(
                "{rel}: exports i18n macros outside the crate via #[macro_export]"
            ));
        }

        let slot_name = match entry.slot {
            ModuleSlot::Layer(c) => c.name(),
            ModuleSlot::ErrorRoot => "error_root",
        };

        for rule in RULES {
            if rule.allowed_slots.contains(&entry.slot) || rule.allowed_files.contains(&rel) {
                continue;
            }
            for &crate_name in rule.crates {
                if entry.refs.external_roots.contains(crate_name) {
                    violations.push(format!(
                        "{rel} ({slot_name}): forbidden external crate `{crate_name}` ({})",
                        rule.rationale
                    ));
                }
            }
            for &marker in rule.bare_markers {
                if entry.refs.bare_markers.contains(marker) {
                    violations.push(format!(
                        "{rel} ({slot_name}): forbidden macro/derive `{marker}` ({})",
                        rule.rationale
                    ));
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "heavy external dependencies escaped their target crate boundaries:\n{}",
        violations.join("\n")
    );
}

fn manifest_dependency_names(manifest_path: &Path) -> BTreeSet<String> {
    manifest_dependency_names_filtered(manifest_path, true)
}

fn manifest_production_dependency_names(manifest_path: &Path) -> BTreeSet<String> {
    manifest_dependency_names_filtered(manifest_path, false)
}

fn manifest_dependency_names_filtered(
    manifest_path: &Path,
    include_dev_and_build: bool,
) -> BTreeSet<String> {
    let contents = fs::read_to_string(manifest_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", manifest_path.display()));
    let mut deps = BTreeSet::new();
    let mut in_deps = false;
    for raw_line in contents.lines() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            let header = &line[1..line.len() - 1];
            in_deps = header == "dependencies"
                || header.ends_with(".dependencies")
                || (include_dev_and_build
                    && (header == "dev-dependencies"
                        || header == "build-dependencies"
                        || header.ends_with(".dev-dependencies")
                        || header.ends_with(".build-dependencies")));
            continue;
        }
        if in_deps && let Some((key, _)) = line.split_once('=') {
            let key = key.trim().trim_matches('"');
            if !key.is_empty() {
                deps.insert(key.to_owned());
            }
        }
    }
    deps
}

#[test]
fn extracted_layer0_manifests_isolate_forbidden_dependencies() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let shared_forbidden = [
        "rust-srec",
        "srec-engines",
        "srec-processors",
        "srec-notify",
        "srec-pipeline",
        "srec-recorder",
        "srec-api",
        "axum",
        "utoipa",
        "utoipa-swagger-ui",
        "schemars",
        "rmcp",
        "lettre",
        "p256",
        "aes-gcm",
        "hkdf",
        "rust-i18n",
        "mesio",
        "mesio-engine",
        "flv",
        "flv-fix",
        "hls",
        "hls-fix",
        "platforms-parser",
        "zip",
        "tar",
        "prost",
        "argon2",
        "jsonwebtoken",
    ];

    let core_deps = manifest_dependency_names(&manifest_dir.join("crates/srec-core/Cargo.toml"));
    let mut violations = Vec::new();
    for forbidden in shared_forbidden.into_iter().chain(["sqlx", "srec-sqlite"]) {
        if core_deps.contains(forbidden) {
            violations.push(format!(
                "crates/srec-core/Cargo.toml must not depend on `{forbidden}`"
            ));
        }
    }

    let sqlite_deps =
        manifest_dependency_names(&manifest_dir.join("crates/srec-sqlite/Cargo.toml"));
    for forbidden in shared_forbidden.into_iter().chain(["reqwest", "rustls"]) {
        if sqlite_deps.contains(forbidden) {
            violations.push(format!(
                "crates/srec-sqlite/Cargo.toml must not depend on `{forbidden}`"
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "extracted Layer 0 Cargo.toml files violated dependency isolation:\n{}",
        violations.join("\n")
    );
}

#[test]
fn extracted_layer1_manifests_isolate_forbidden_dependencies() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let common_forbidden = [
        "rust-srec",
        "srec-pipeline",
        "srec-recorder",
        "srec-api",
        "platforms-parser",
        "axum",
        "utoipa",
        "utoipa-swagger-ui",
        "schemars",
        "rmcp",
        "prost",
        "argon2",
        "jsonwebtoken",
    ];
    let notify_only_deps = [
        "sqlx",
        "srec-sqlite",
        "srec-notify",
        "lettre",
        "p256",
        "aes-gcm",
        "hkdf",
        "rust-i18n",
    ];
    let engine_only_deps = [
        "srec-engines",
        "mesio",
        "mesio-engine",
        "flv",
        "flv-fix",
        "hls",
        "hls-fix",
    ];
    let processor_only_deps = ["srec-processors", "zip", "tar"];

    let mut violations = Vec::new();

    let processors_deps =
        manifest_dependency_names(&manifest_dir.join("crates/srec-processors/Cargo.toml"));
    for forbidden in common_forbidden
        .into_iter()
        .chain(notify_only_deps)
        .chain(engine_only_deps)
    {
        if processors_deps.contains(forbidden) {
            violations.push(format!(
                "crates/srec-processors/Cargo.toml must not depend on `{forbidden}`"
            ));
        }
    }

    let engines_deps =
        manifest_dependency_names(&manifest_dir.join("crates/srec-engines/Cargo.toml"));
    for forbidden in common_forbidden
        .into_iter()
        .chain(notify_only_deps)
        .chain(processor_only_deps)
    {
        if engines_deps.contains(forbidden) {
            violations.push(format!(
                "crates/srec-engines/Cargo.toml must not depend on `{forbidden}`"
            ));
        }
    }

    let notify_deps =
        manifest_dependency_names(&manifest_dir.join("crates/srec-notify/Cargo.toml"));
    for forbidden in common_forbidden
        .into_iter()
        .chain(engine_only_deps)
        .chain(processor_only_deps)
    {
        if notify_deps.contains(forbidden) {
            violations.push(format!(
                "crates/srec-notify/Cargo.toml must not depend on `{forbidden}`"
            ));
        }
    }

    let root_deps = manifest_dependency_names(&manifest_dir.join("Cargo.toml"));
    for notify_exclusive in ["lettre", "p256", "aes-gcm", "hkdf", "rust-i18n"] {
        if root_deps.contains(notify_exclusive) {
            violations.push(format!(
                "rust-srec/Cargo.toml must not depend directly on `{notify_exclusive}` (belongs exclusively to srec-notify)"
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "extracted Layer 1 Cargo.toml files violated dependency isolation:\n{}",
        violations.join("\n")
    );
}

#[test]
fn extracted_layer2_manifests_isolate_forbidden_dependencies() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let pipeline_forbidden = [
        "rust-srec",
        "srec-recorder",
        "srec-engines",
        "srec-notify",
        "srec-api",
        "platforms-parser",
        "mesio",
        "mesio-engine",
        "flv",
        "flv-fix",
        "hls",
        "hls-fix",
        "axum",
        "utoipa",
        "utoipa-swagger-ui",
        "schemars",
        "rmcp",
        "lettre",
        "p256",
        "aes-gcm",
        "hkdf",
        "rust-i18n",
        "prost",
        "argon2",
        "jsonwebtoken",
        "zip",
        "tar",
    ];

    let pipeline_deps =
        manifest_dependency_names(&manifest_dir.join("crates/srec-pipeline/Cargo.toml"));
    let mut violations = Vec::new();
    for forbidden in pipeline_forbidden {
        if pipeline_deps.contains(forbidden) {
            violations.push(format!(
                "crates/srec-pipeline/Cargo.toml must not depend on `{forbidden}`"
            ));
        }
    }

    let recorder_forbidden = [
        "rust-srec",
        "srec-pipeline",
        "srec-processors",
        "srec-notify",
        "srec-api",
        "zip",
        "tar",
        "axum",
        "utoipa",
        "utoipa-swagger-ui",
        "schemars",
        "rmcp",
        "lettre",
        "p256",
        "aes-gcm",
        "hkdf",
        "rust-i18n",
        "prost",
        "argon2",
        "jsonwebtoken",
    ];
    let recorder_deps =
        manifest_dependency_names(&manifest_dir.join("crates/srec-recorder/Cargo.toml"));
    for forbidden in recorder_forbidden {
        if recorder_deps.contains(forbidden) {
            violations.push(format!(
                "crates/srec-recorder/Cargo.toml must not depend on `{forbidden}`"
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "extracted Layer 2 Cargo.toml files violated dependency isolation:\n{}",
        violations.join("\n")
    );
}

#[test]
fn extracted_layer3_and_root_manifests_isolate_forbidden_dependencies() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut violations = Vec::new();

    // `srec-api` must never depend on root `rust-srec` (even in dev-dependencies).
    let api_all_deps = manifest_dependency_names(&manifest_dir.join("crates/srec-api/Cargo.toml"));
    if api_all_deps.contains("rust-srec") {
        violations.push("crates/srec-api/Cargo.toml must not depend on `rust-srec`".to_owned());
    }

    // `srec-api` production dependencies must not absorb Layer 1 exclusive crates.
    let api_prod_deps =
        manifest_production_dependency_names(&manifest_dir.join("crates/srec-api/Cargo.toml"));
    for forbidden in [
        "mesio",
        "mesio-engine",
        "flv",
        "flv-fix",
        "hls",
        "hls-fix",
        "zip",
        "tar",
        "lettre",
        "p256",
        "aes-gcm",
        "hkdf",
        "rust-i18n",
        "prost-build",
        "protox",
    ] {
        if api_prod_deps.contains(forbidden) {
            violations.push(format!(
                "crates/srec-api/Cargo.toml [dependencies] must not depend on `{forbidden}`"
            ));
        }
    }

    // Root `rust-srec` production `[dependencies]` stays a thin composition shell:
    // API, MCP, OpenAPI, auth crypto, Protobuf, archive, notification, and media
    // engine crates are owned exclusively by their respective workspace crates.
    let root_prod_deps = manifest_production_dependency_names(&manifest_dir.join("Cargo.toml"));
    for forbidden in [
        "axum",
        "tower",
        "tower-http",
        "utoipa",
        "utoipa-swagger-ui",
        "rmcp",
        "schemars",
        "jsonwebtoken",
        "argon2",
        "prost",
        "prost-build",
        "protox",
        "tracing-appender",
        "tracing-subscriber",
        "arc-swap",
        "lettre",
        "p256",
        "aes-gcm",
        "hkdf",
        "rust-i18n",
        "mesio",
        "mesio-engine",
        "flv",
        "flv-fix",
        "hls",
        "hls-fix",
        "zip",
        "tar",
    ] {
        if root_prod_deps.contains(forbidden) {
            violations.push(format!(
                "rust-srec/Cargo.toml [dependencies] must not depend directly on `{forbidden}`"
            ));
        }
    }

    for extracted_module in ["api", "mcp", "logging", "metrics"] {
        let src_dir = manifest_dir.join("src");
        if src_dir.join(format!("{extracted_module}.rs")).exists()
            || src_dir.join(extracted_module).exists()
        {
            violations.push(format!(
                "rust-srec/src/{extracted_module} must remain extracted in crates/srec-api"
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "Layer 3 (srec-api) and Layer 4 (rust-srec) manifest isolation violated:\n{}",
        violations.join("\n")
    );
}

#[test]
fn desktop_tauri_accesses_backend_only_through_facade() {
    let tauri_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src-tauri/src");
    let mut files = Vec::new();
    rust_files(&tauri_src, &mut files);
    files.sort();
    assert!(
        !files.is_empty(),
        "expected Rust source files under {}",
        tauri_src.display()
    );

    let mut facade_uses = 0usize;
    let mut violations = Vec::new();

    for file in &files {
        // Scan both production and test code in src-tauri so desktop tests stay
        // decoupled from internal backend modules as well.
        let refs = extract_file_refs(&tauri_src, file, false);
        let rel = file.strip_prefix(&tauri_src).unwrap_or(file);
        for path in &refs.rust_srec_paths {
            if path.get(1).is_some_and(|segment| segment == "backend") {
                facade_uses += 1;
            } else {
                violations.push(format!(
                    "src-tauri/src/{}: direct internal reference `{}` (must go through rust_srec::backend)",
                    rel.display(),
                    path.join("::")
                ));
            }
        }
        for internal_crate in [
            "srec_core",
            "srec_sqlite",
            "srec_engines",
            "srec_processors",
            "srec_notify",
            "srec_pipeline",
            "srec_recorder",
            "srec_api",
        ] {
            if refs.external_roots.contains(internal_crate) {
                violations.push(format!(
                    "src-tauri/src/{}: direct reference to `{internal_crate}` (must go through rust_srec::backend)",
                    rel.display()
                ));
            }
        }
    }

    let tauri_deps = manifest_dependency_names(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src-tauri/Cargo.toml"),
    );
    for internal_crate in [
        "srec-core",
        "srec-sqlite",
        "srec-engines",
        "srec-processors",
        "srec-notify",
        "srec-pipeline",
        "srec-recorder",
        "srec-api",
    ] {
        if tauri_deps.contains(internal_crate) {
            violations.push(format!(
                "src-tauri/Cargo.toml must not depend directly on `{internal_crate}` (must go through rust-srec::backend)"
            ));
        }
    }

    assert!(
        facade_uses > 0,
        "expected src-tauri/src to reference rust_srec::backend; path extraction may be broken"
    );
    assert!(
        violations.is_empty(),
        "rust-srec-desktop must access rust_srec exclusively through rust_srec::backend:\n{}",
        violations.join("\n")
    );
}
