use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::Value;
use syn::parse::Parser;
use syn::visit::Visit;
use syn::{Expr, Item, ItemFn, Lit, Meta, Token, UseTree};
use utoipa::OpenApi;

use super::ApiDoc;

#[derive(Default)]
struct Sources {
    functions: BTreeMap<String, ItemFn>,
    imports: BTreeMap<String, BTreeMap<String, Vec<String>>>,
}

fn use_names(tree: &UseTree, prefix: Vec<String>, names: &mut BTreeMap<String, Vec<String>>) {
    match tree {
        UseTree::Path(path) => {
            let mut prefix = prefix;
            prefix.push(path.ident.to_string());
            use_names(&path.tree, prefix, names);
        }
        UseTree::Name(name) => {
            let mut path = prefix;
            path.push(name.ident.to_string());
            names.insert(name.ident.to_string(), path);
        }
        UseTree::Rename(rename) => {
            let mut path = prefix;
            path.push(rename.ident.to_string());
            names.insert(rename.rename.to_string(), path);
        }
        UseTree::Group(group) => {
            for item in &group.items {
                use_names(item, prefix.clone(), names);
            }
        }
        UseTree::Glob(_) => {}
    }
}

impl Sources {
    fn load(&mut self, file: &Path, module: &str) {
        let source = std::fs::read_to_string(file).unwrap();
        let syntax = syn::parse_file(&source).unwrap();
        let directory = file.with_extension("");
        let mut imports = BTreeMap::new();
        for item in syntax.items {
            match item {
                Item::Fn(function) => {
                    self.functions.insert(format!("{module}::{}", function.sig.ident), function);
                }
                Item::Use(import) => use_names(&import.tree, Vec::new(), &mut imports),
                Item::Mod(child) if child.content.is_none() && !child.attrs.iter().any(|attribute| {
                    attribute.path().is_ident("cfg") && matches!(&attribute.meta, Meta::List(list) if list.tokens.to_string().contains("test"))
                }) => self.load(&directory.join(format!("{}.rs", child.ident)), &format!("{module}::{}", child.ident)),
                _ => {}
            }
        }
        self.imports.insert(module.to_owned(), imports);
    }

    fn resolve(&self, module: &str, path: &syn::Path) -> String {
        let mut parts: Vec<_> = path
            .segments
            .iter()
            .map(|part| part.ident.to_string())
            .collect();
        if let Some(import) = self
            .imports
            .get(module)
            .and_then(|imports| imports.get(&parts[0]))
        {
            let mut expanded = import.clone();
            expanded.extend(parts.into_iter().skip(1));
            parts = expanded;
        }
        let mut resolved: Vec<_> = module.split("::").map(str::to_owned).collect();
        if parts.first().is_some_and(|part| part == "crate") {
            return parts.join("::");
        }
        while parts
            .first()
            .is_some_and(|part| part == "super" || part == "self")
        {
            if parts.remove(0) == "super" {
                resolved.pop();
            }
        }
        resolved.extend(parts);
        resolved.join("::")
    }

    fn routed_handlers(
        &self,
        function: &str,
        prefix: &str,
        routes: &mut BTreeMap<String, BTreeSet<(String, String)>>,
    ) {
        let module = function.rsplit_once("::").unwrap().0;
        let function_body = self
            .functions
            .get(function)
            .unwrap_or_else(|| panic!("Unresolved router {function}"));
        let mut registrations = Registrations::default();
        registrations.visit_block(&function_body.block);
        for (path, method, handler) in registrations.routes {
            let handler = self.resolve(module, &handler);
            let path = format!("{prefix}{path}").trim_end_matches('/').to_owned();
            routes.entry(handler).or_default().insert((path, method));
        }
        for (mount, router) in registrations.mounts {
            let router = self.resolve(module, &router);
            if self.functions.contains_key(&router) {
                self.routed_handlers(&router, &format!("{prefix}{mount}"), routes);
            }
        }
    }
}

fn string_literal(expression: &Expr) -> Option<String> {
    match expression {
        Expr::Lit(literal) => match &literal.lit {
            Lit::Str(value) => Some(value.value()),
            _ => None,
        },
        _ => None,
    }
}

fn http_method(name: &str) -> bool {
    matches!(
        name,
        "get" | "post" | "put" | "patch" | "delete" | "head" | "options" | "trace"
    )
}

#[derive(Default)]
struct Registrations {
    routes: Vec<(String, String, syn::Path)>,
    mounts: Vec<(String, syn::Path)>,
}

impl Registrations {
    fn handlers(&mut self, route: &str, expression: &Expr) {
        let (method, arguments) = match expression {
            Expr::Call(call) => match call.func.as_ref() {
                Expr::Path(path) => (
                    path.path.segments.last().unwrap().ident.to_string(),
                    &call.args,
                ),
                _ => return,
            },
            Expr::MethodCall(call) => {
                self.handlers(route, &call.receiver);
                (call.method.to_string(), &call.args)
            }
            _ => return,
        };
        if http_method(&method) {
            let Some(Expr::Path(handler)) = arguments.first() else {
                panic!("Unrecognized {method} handler for {route}");
            };
            self.routes
                .push((route.to_owned(), method, handler.path.clone()));
        }
    }
}

impl<'ast> Visit<'ast> for Registrations {
    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        if call.method == "route" {
            let path =
                string_literal(&call.args[0]).expect("Route paths must be inspected explicitly");
            self.handlers(&path, &call.args[1]);
        } else if call.method == "nest" || call.method == "merge" {
            let (prefix, expression) = if call.method == "nest" {
                (string_literal(&call.args[0]).unwrap(), &call.args[1])
            } else {
                (String::new(), &call.args[0])
            };
            if let Expr::Call(router) = expression
                && let Expr::Path(path) = router.func.as_ref()
            {
                self.mounts.push((prefix, path.path.clone()));
            }
        }
        syn::visit::visit_expr_method_call(self, call);
    }
}

#[test]
fn every_annotated_handler_is_mounted_and_registered_with_its_method() {
    // This is the annotated REST contract. Unannotated WebSocket/proxy handlers,
    // the MCP service and framework-owned Swagger routes have separate protocols;
    // discovering their mounts must not invent REST operations for them.
    let mut sources = Sources::default();
    sources.load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src/api/routes.rs"),
        "crate::api::routes",
    );
    let mut routes = BTreeMap::new();
    sources.routed_handlers("crate::api::routes::create_router", "", &mut routes);
    let document = serde_json::to_value(ApiDoc::openapi()).unwrap();
    let parser = syn::punctuated::Punctuated::<Meta, Token![,]>::parse_terminated;
    let mut annotated = 0;
    for (handler, function) in &sources.functions {
        for attribute in &function.attrs {
            if attribute
                .path()
                .segments
                .iter()
                .map(|part| part.ident.to_string())
                .collect::<Vec<_>>()
                != ["utoipa", "path"]
            {
                continue;
            }
            let Meta::List(attribute) = &attribute.meta else {
                unreachable!()
            };
            let arguments = parser.parse2(attribute.tokens.clone()).unwrap();
            let mut method = None;
            let mut path = None;
            for argument in arguments {
                match argument {
                    Meta::Path(name)
                        if name
                            .get_ident()
                            .is_some_and(|name| http_method(&name.to_string())) =>
                    {
                        method = name.get_ident().map(ToString::to_string)
                    }
                    Meta::NameValue(value) if value.path.is_ident("path") => {
                        path = string_literal(&value.value)
                    }
                    _ => {}
                }
            }
            let pair = (
                path.expect("documented path"),
                method.expect("documented method"),
            );
            assert!(
                routes
                    .get(handler)
                    .is_some_and(|registered| registered.contains(&pair)),
                "{handler}: documented {pair:?} is not mounted by the application router"
            );
            assert!(
                document["paths"][&pair.0][&pair.1].is_object(),
                "{handler}: {pair:?} is missing from OpenAPI"
            );
            annotated += 1;
        }
    }
    assert!(
        annotated > 100,
        "source discovery unexpectedly lost route modules"
    );
}

fn assert_schema_references(value: &Value, document: &Value) {
    match value {
        Value::Object(object) => {
            if let Some(Value::String(reference)) = object.get("$ref")
                && let Some(pointer) = reference.strip_prefix('#')
            {
                assert!(
                    document.pointer(pointer).is_some(),
                    "Missing OpenAPI schema {reference}"
                );
            }
            for value in object.values() {
                assert_schema_references(value, document);
            }
        }
        Value::Array(array) => {
            for value in array {
                assert_schema_references(value, document);
            }
        }
        _ => {}
    }
}

#[test]
fn documented_requests_and_responses_resolve_every_component_reference() {
    let document = serde_json::to_value(ApiDoc::openapi()).unwrap();
    assert_schema_references(&document, &document);
}
