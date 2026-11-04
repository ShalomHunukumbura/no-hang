//! Python: find calls into known libraries and check how their timeout is set.
//!
//! The file is parsed with tree-sitter, so comments, strings and docstrings that mention
//! `requests.get(` are never mistaken for calls, and `import requests as r` or
//! `from requests import post` are followed to the real function.
//!
//! Clients are followed too: `s = requests.Session()` then `s.get(url)`, `with
//! httpx.Client() as c`, `self.session = aiohttp.ClientSession()` in `__init__` and
//! `self.session.get(url)` in another method, or a parameter typed `requests.Session`.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};

use tree_sitter::{Node, Parser};

use crate::finding::Finding;
use crate::rules::{self, CANT_CHECK, HTTP_NO_TIMEOUT, SUBPROCESS_NO_TIMEOUT, TIMEOUT_NONE};

const HTTP_VERBS: &[&str] = &[
    "get", "post", "put", "patch", "delete", "head", "options", "request",
];

/// A module-level function that blocks, and the keyword argument that bounds it.
struct Function {
    module: &'static str,
    functions: &'static [&'static str],
    param: &'static str,
    /// The rule for leaving the timeout out, or None when the library has a default
    /// (then only an explicit `timeout=None` is a finding).
    missing: Option<&'static str>,
}

/// What each library does when no timeout is passed is the knowledge these tables hold.
const FUNCTIONS: &[Function] = &[
    Function {
        module: "requests",
        functions: HTTP_VERBS,
        param: "timeout",
        missing: Some(HTTP_NO_TIMEOUT),
    },
    Function {
        module: "subprocess",
        functions: &["run", "call", "check_call", "check_output"],
        param: "timeout",
        missing: Some(SUBPROCESS_NO_TIMEOUT),
    },
    // httpx times out after 5 seconds unless told otherwise.
    Function {
        module: "httpx",
        functions: &[
            "get", "post", "put", "patch", "delete", "head", "options", "request", "stream",
        ],
        param: "timeout",
        missing: None,
    },
    // aiohttp.request uses the 5 minute session default.
    Function {
        module: "aiohttp",
        functions: &["request"],
        param: "timeout",
        missing: None,
    },
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Library {
    Requests,
    Httpx,
    Aiohttp,
}

impl Library {
    fn methods(self) -> &'static [&'static str] {
        match self {
            Library::Requests => HTTP_VERBS,
            Library::Httpx => &[
                "get", "post", "put", "patch", "delete", "head", "options", "request", "stream",
            ],
            Library::Aiohttp => &[
                "get",
                "post",
                "put",
                "patch",
                "delete",
                "head",
                "options",
                "request",
                "ws_connect",
            ],
        }
    }
}

/// Classes whose instances make requests.
const CLIENTS: &[(&str, Library)] = &[
    ("requests.Session", Library::Requests),
    ("requests.session", Library::Requests),
    ("requests.sessions.Session", Library::Requests),
    ("httpx.Client", Library::Httpx),
    ("httpx.AsyncClient", Library::Httpx),
    ("aiohttp.ClientSession", Library::Aiohttp),
];

fn client_class(qualified: &str) -> Option<Library> {
    CLIENTS
        .iter()
        .find(|(name, _)| *name == qualified)
        .map(|(_, lib)| *lib)
}

/// Names a file binds by importing.
#[derive(Default, Debug)]
struct Imports {
    /// `import requests as r` binds r -> requests.
    modules: HashMap<String, String>,
    /// `from requests import post as p` binds p -> requests.post.
    names: HashMap<String, String>,
}

/// A name in a scope: a function (or the module) for plain names, a class for `self.x`.
type Key = (usize, String);

/// What every assignment in the file binds each name to.
#[derive(Default, Debug)]
struct Bindings {
    /// Every value a name is given; `None` is anything that isn't a client we know.
    values: HashMap<Key, Vec<Option<Library>>>,
    /// Sessions given an adapter or a wrapped `request`, which may set a timeout we can't see.
    customised: HashSet<Key>,
    /// Sessions with `s.timeout = ...`, which requests ignores.
    timeout_attr: HashSet<Key>,
}

struct Ctx<'a> {
    src: &'a [u8],
    path: &'a str,
    root: Node<'a>,
    imports: Imports,
    bindings: Bindings,
    /// Each node's parent. tree-sitter's `Node::parent` searches down from the root, which
    /// is slow on big files, so the parents are recorded once.
    parents: Parents<'a>,
}

impl<'a> Ctx<'a> {
    fn parent(&self, node: Node<'a>) -> Option<Node<'a>> {
        self.parents.get(&node.id()).copied()
    }

    /// The node's ancestors, innermost first.
    fn ancestors(&self, node: Node<'a>) -> impl Iterator<Item = Node<'a>> + '_ {
        std::iter::successors(self.parent(node), |n| self.parent(*n))
    }
}

/// Every finding needs one of these modules imported, so a file that never mentions any of
/// them is skipped without parsing it. Most files in a big project are.
const MODULES: &[&str] = &["requests", "subprocess", "httpx", "aiohttp"];

pub fn check(path: &str, source: &str) -> Vec<Finding> {
    if !MODULES.iter().any(|m| source.contains(m)) {
        return Vec::new();
    }
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .expect("the Python grammar matches the tree-sitter version");
    let Some(tree) = parser.parse(source, None) else {
        return Vec::new();
    };
    let src = source.as_bytes();
    let root = tree.root_node();

    let mut ctx = Ctx {
        src,
        path,
        root,
        imports: Imports::default(),
        bindings: Bindings::default(),
        parents: Parents::default(),
    };
    let nodes = flatten(root, &mut ctx.parents);
    for node in &nodes {
        collect_import(*node, src, &mut ctx.imports);
    }
    let mut bindings = Bindings::default();
    for node in &nodes {
        collect_binding(*node, &ctx, &mut bindings);
    }
    ctx.bindings = bindings;

    nodes
        .iter()
        .filter(|n| n.kind() == "call")
        .filter_map(|n| check_call(*n, &ctx))
        .collect()
}

/// Node ids are addresses: already unique, so a full hash is wasted work. They're aligned,
/// so the low bits are always zero; one multiply and fold spreads them.
#[derive(Default)]
struct IdHasher(u64);

impl Hasher for IdHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, _: &[u8]) {
        unreachable!("only usize keys are hashed")
    }
    fn write_usize(&mut self, n: usize) {
        let h = (n as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        self.0 = h ^ (h >> 32);
    }
}

type Parents<'t> = HashMap<usize, Node<'t>, BuildHasherDefault<IdHasher>>;

/// Every node in document order, recording each one's parent on the way.
fn flatten<'t>(root: Node<'t>, parents: &mut Parents<'t>) -> Vec<Node<'t>> {
    let count = root.descendant_count();
    parents.reserve(count);
    let mut nodes = Vec::with_capacity(count);
    // The ancestors of the cursor's node, innermost last.
    let mut path: Vec<Node<'t>> = Vec::new();
    let mut cursor = root.walk();
    loop {
        let node = cursor.node();
        nodes.push(node);
        if let Some(parent) = path.last() {
            parents.insert(node.id(), *parent);
        }
        if cursor.goto_first_child() {
            path.push(node);
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return nodes;
            }
            path.pop();
        }
    }
}

/// Visit every node under `node`, depth first. For small subtrees like assignment targets.
fn walk<'t>(node: Node<'t>, visit: &mut impl FnMut(Node<'t>)) {
    visit(node);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(child, visit);
    }
}

fn text<'s>(node: Node, src: &'s [u8]) -> &'s str {
    node.utf8_text(src).unwrap_or("")
}

fn collect_import(node: Node, src: &[u8], imports: &mut Imports) {
    match node.kind() {
        "import_statement" => {
            let mut cursor = node.walk();
            for name in node.children_by_field_name("name", &mut cursor) {
                match name.kind() {
                    // `import os.path` binds `os`.
                    "dotted_name" => {
                        let full = text(name, src);
                        let first = full.split('.').next().unwrap_or(full);
                        imports.modules.insert(first.to_string(), first.to_string());
                    }
                    "aliased_import" => {
                        if let (Some(n), Some(a)) = (
                            name.child_by_field_name("name"),
                            name.child_by_field_name("alias"),
                        ) {
                            imports
                                .modules
                                .insert(text(a, src).into(), text(n, src).into());
                        }
                    }
                    _ => {}
                }
            }
        }
        "import_from_statement" => {
            let Some(module) = node.child_by_field_name("module_name") else {
                return;
            };
            // Relative imports are the project's own code, not a library.
            if module.kind() != "dotted_name" {
                return;
            }
            let module = text(module, src);
            let mut cursor = node.walk();
            for name in node.children_by_field_name("name", &mut cursor) {
                let (local, real) = match name.kind() {
                    "dotted_name" => (text(name, src), text(name, src)),
                    "aliased_import" => match (
                        name.child_by_field_name("name"),
                        name.child_by_field_name("alias"),
                    ) {
                        (Some(n), Some(a)) => (text(a, src), text(n, src)),
                        _ => continue,
                    },
                    _ => continue,
                };
                imports
                    .names
                    .insert(local.to_string(), format!("{module}.{real}"));
            }
        }
        _ => {}
    }
}

/// The dotted name an expression refers to, with imports resolved:
/// `r.post` after `import requests as r` is `requests.post`.
fn resolve(expr: Node, src: &[u8], imports: &Imports) -> Option<String> {
    match expr.kind() {
        "identifier" => imports.names.get(text(expr, src)).cloned(),
        "attribute" => {
            let mut parts = vec![text(expr.child_by_field_name("attribute")?, src)];
            let mut object = expr.child_by_field_name("object")?;
            while object.kind() == "attribute" {
                parts.push(text(object.child_by_field_name("attribute")?, src));
                object = object.child_by_field_name("object")?;
            }
            if object.kind() != "identifier" {
                return None;
            }
            let base = text(object, src);
            let base = imports
                .modules
                .get(base)
                .or_else(|| imports.names.get(base))?;
            parts.reverse();
            Some(format!("{base}.{}", parts.join(".")))
        }
        _ => None,
    }
}

/// The client class an expression constructs, for `requests.Session()` and the like.
fn constructs(expr: Node, ctx: &Ctx) -> Option<Library> {
    if expr.kind() != "call" {
        return None;
    }
    let qualified = resolve(expr.child_by_field_name("function")?, ctx.src, &ctx.imports)?;
    client_class(&qualified)
}

/// The scope a name lives in: the innermost function, or the module.
fn function_scope(node: Node, ctx: &Ctx) -> usize {
    ctx.ancestors(node)
        .find(|n| matches!(n.kind(), "function_definition" | "lambda"))
        .map_or(ctx.root.id(), |n| n.id())
}

fn class_scope(node: Node, ctx: &Ctx) -> Option<usize> {
    ctx.ancestors(node)
        .find(|n| n.kind() == "class_definition")
        .map(|n| n.id())
}

/// The binding key for an expression that can hold a client: a plain name, or `self.x`.
fn key_of(expr: Node, ctx: &Ctx) -> Option<Key> {
    match expr.kind() {
        "identifier" => Some((function_scope(expr, ctx), text(expr, ctx.src).to_string())),
        "attribute" => {
            let object = expr.child_by_field_name("object")?;
            if object.kind() != "identifier" || !matches!(text(object, ctx.src), "self" | "cls") {
                return None;
            }
            let attr = text(expr.child_by_field_name("attribute")?, ctx.src);
            Some((class_scope(expr, ctx)?, format!("self.{attr}")))
        }
        _ => None,
    }
}

fn collect_binding(node: Node, ctx: &Ctx, b: &mut Bindings) {
    let mut bind = |target: Node, value: Option<Library>| {
        if let Some(key) = key_of(target, ctx) {
            b.values.entry(key).or_default().push(value);
        }
    };
    match node.kind() {
        "assignment" | "named_expression" => {
            let (left, right) = if node.kind() == "assignment" {
                (
                    node.child_by_field_name("left"),
                    node.child_by_field_name("right"),
                )
            } else {
                (
                    node.child_by_field_name("name"),
                    node.child_by_field_name("value"),
                )
            };
            let Some(left) = left else { return };
            let value = match right {
                Some(r) => constructs(r, ctx),
                // `self.s: requests.Session` with no value declares, it doesn't bind.
                None => return,
            };
            match left.kind() {
                "identifier" => bind(left, value),
                "attribute" => {
                    // `s.mount(...)` and `s.request = ...` may add a timeout we can't see.
                    if let Some(object) = left.child_by_field_name("object") {
                        let attr = left
                            .child_by_field_name("attribute")
                            .map(|a| text(a, ctx.src));
                        if let Some(key) = key_of(object, ctx) {
                            match attr {
                                Some("timeout") => {
                                    b.timeout_attr.insert(key);
                                }
                                Some(a) if HTTP_VERBS.contains(&a) || a == "send" => {
                                    b.customised.insert(key);
                                }
                                _ => {}
                            }
                        }
                    }
                    bind(left, value)
                }
                // `a, b = ...`: whatever they get, it isn't known here.
                _ => walk(left, &mut |n| {
                    if n.kind() == "identifier" {
                        bind(n, None)
                    }
                }),
            }
        }
        "as_pattern" => {
            let value = node.child(0).and_then(|v| constructs(v, ctx));
            if let Some(target) = node
                .child_by_field_name("alias")
                .and_then(|a| a.named_child(0))
            {
                bind(target, value);
            }
        }
        "for_statement" => {
            if let Some(left) = node.child_by_field_name("left") {
                walk(left, &mut |n| {
                    if n.kind() == "identifier" {
                        bind(n, None)
                    }
                });
            }
        }
        "parameters" | "lambda_parameters" => {
            let mut cursor = node.walk();
            for param in node.named_children(&mut cursor) {
                let (name, typ) = match param.kind() {
                    "identifier" => (Some(param), None),
                    "typed_parameter" => (param.named_child(0), param.child_by_field_name("type")),
                    "default_parameter" => (param.child_by_field_name("name"), None),
                    "typed_default_parameter" => (
                        param.child_by_field_name("name"),
                        param.child_by_field_name("type"),
                    ),
                    _ => continue,
                };
                let Some(name) = name.filter(|n| n.kind() == "identifier") else {
                    continue;
                };
                // A parameter belongs to the function it's declared on.
                let value = typ
                    .and_then(|t| t.named_child(0))
                    .and_then(|t| resolve(t, ctx.src, &ctx.imports))
                    .and_then(|q| client_class(&q));
                let scope = param_scope(node, ctx);
                b.values
                    .entry((scope, text(name, ctx.src).to_string()))
                    .or_default()
                    .push(value);
            }
        }
        "call" => {
            // `s.mount("https://", adapter)`: the adapter may set a default timeout.
            let Some(function) = node.child_by_field_name("function") else {
                return;
            };
            if function.kind() != "attribute" {
                return;
            }
            let method = function
                .child_by_field_name("attribute")
                .map(|a| text(a, ctx.src));
            if method == Some("mount") {
                if let Some(key) = function
                    .child_by_field_name("object")
                    .and_then(|o| key_of(o, ctx))
                {
                    b.customised.insert(key);
                }
            }
        }
        _ => {}
    }
}

fn param_scope(parameters: Node, ctx: &Ctx) -> usize {
    ctx.parent(parameters)
        .filter(|p| matches!(p.kind(), "function_definition" | "lambda"))
        .map_or(ctx.root.id(), |p| p.id())
}

/// The client an expression holds, if every binding of it in scope is the same client.
fn client_of(expr: Node, ctx: &Ctx) -> Option<(Library, Option<Key>)> {
    if let Some(lib) = constructs(expr, ctx) {
        return Some((lib, None));
    }
    let key = key_of(expr, ctx)?;
    let key = if expr.kind() == "identifier" {
        lookup_name(expr, &key.1, ctx)?
    } else {
        key
    };
    if ctx.bindings.customised.contains(&key) {
        return None;
    }
    let values = ctx.bindings.values.get(&key)?;
    let first = (*values.first()?)?;
    values
        .iter()
        .all(|v| *v == Some(first))
        .then_some((first, Some(key)))
}

/// Find the scope that binds a plain name: the innermost function that assigns it, or the
/// module.
fn lookup_name(expr: Node, name: &str, ctx: &Ctx) -> Option<Key> {
    for n in ctx.ancestors(expr) {
        if matches!(n.kind(), "function_definition" | "lambda") {
            let key = (n.id(), name.to_string());
            if ctx.bindings.values.contains_key(&key) {
                return Some(key);
            }
        }
    }
    let key = (ctx.root.id(), name.to_string());
    ctx.bindings.values.contains_key(&key).then_some(key)
}

fn check_call(call: Node, ctx: &Ctx) -> Option<Finding> {
    let function = call.child_by_field_name("function")?;
    let arguments = call.child_by_field_name("arguments")?;
    let shown = text(function, ctx.src);

    if let Some(qualified) = resolve(function, ctx.src, &ctx.imports) {
        if let Some(f) = function_for(&qualified) {
            let message = match f.missing {
                Some(SUBPROCESS_NO_TIMEOUT) => format!(
                    "{shown}() has no timeout, so a command that never exits blocks forever."
                ),
                _ => format!(
                    "{shown}() has no timeout, so it waits forever if the server stops answering."
                ),
            };
            return check_arguments(
                arguments,
                f.param,
                f.missing.map(|r| (r, call, message)),
                ctx,
                shown,
            );
        }
        return check_constructor(&qualified, call, ctx);
    }

    // A method on a client: `s.get(url)`.
    if function.kind() != "attribute" {
        return None;
    }
    let method = text(function.child_by_field_name("attribute")?, ctx.src);
    let (lib, key) = client_of(function.child_by_field_name("object")?, ctx)?;
    if !lib.methods().contains(&method) {
        return None;
    }
    let missing = (lib == Library::Requests).then(|| {
        let mut message = format!(
            "{shown}() has no timeout, and a requests.Session has no default, so it waits \
             forever if the server stops answering."
        );
        if key.is_some_and(|k| ctx.bindings.timeout_attr.contains(&k)) {
            message.push_str(" Setting the session's .timeout does nothing in requests.");
        }
        (HTTP_NO_TIMEOUT, call, message)
    });
    check_arguments(arguments, "timeout", missing, ctx, shown)
}

fn function_for(qualified: &str) -> Option<&'static Function> {
    let (module, function) = qualified.rsplit_once('.')?;
    FUNCTIONS
        .iter()
        .find(|f| f.module == module && f.functions.contains(&function))
}

/// Check the timeout argument of a call. `missing` is the finding to give when there's
/// none, for libraries with no default.
fn check_arguments(
    arguments: Node,
    param: &str,
    missing: Option<(&'static str, Node, String)>,
    ctx: &Ctx,
    shown: &str,
) -> Option<Finding> {
    let mut splat = None;
    let mut cursor = arguments.walk();
    for arg in arguments.named_children(&mut cursor) {
        match arg.kind() {
            "keyword_argument" => {
                if keyword(arg, ctx) == Some(param) {
                    let value = arg.child_by_field_name("value")?;
                    if value.kind() == "none" {
                        return Some(finding(
                            ctx,
                            value,
                            TIMEOUT_NONE,
                            format!("{shown}({param}=None) turns the timeout off."),
                        ));
                    }
                    return None;
                }
            }
            "dictionary_splat" => splat = Some(arg),
            _ => {}
        }
    }

    let (rule, node, message) = missing?;
    if let Some(splat) = splat {
        return Some(finding(
            ctx,
            splat,
            CANT_CHECK,
            format!(
                "{shown}({}) may set {param} inside the dictionary, so it can't be checked.",
                text(splat, ctx.src)
            ),
        ));
    }
    Some(finding(ctx, node, rule, message))
}

fn keyword<'s>(arg: Node, ctx: &Ctx<'s>) -> Option<&'s str> {
    arg.child_by_field_name("name").map(|n| text(n, ctx.src))
}

/// Constructors whose arguments turn timeouts off for everything that uses them.
fn check_constructor(qualified: &str, call: Node, ctx: &Ctx) -> Option<Finding> {
    let arguments = call.child_by_field_name("arguments")?;
    let shown = text(call.child_by_field_name("function")?, ctx.src);
    let mut positional = Vec::new();
    let mut keywords = HashMap::new();
    let mut cursor = arguments.walk();
    for arg in arguments.named_children(&mut cursor) {
        match arg.kind() {
            "keyword_argument" => {
                if let (Some(name), Some(value)) =
                    (keyword(arg, ctx), arg.child_by_field_name("value"))
                {
                    keywords.insert(name, value);
                }
            }
            // Anything could be in there.
            "dictionary_splat" | "list_splat" => return None,
            "comment" => {}
            _ => positional.push(arg),
        }
    }
    let is_none = |n: &Node| n.kind() == "none";

    match qualified {
        // aiohttp.ClientSession(timeout=None) means the 5 minute default, so only httpx here.
        "httpx.Client" | "httpx.AsyncClient" => {
            let value = keywords.get("timeout").filter(|v| is_none(v))?;
            Some(finding(
                ctx,
                *value,
                TIMEOUT_NONE,
                format!("{shown}(timeout=None) turns the timeout off for every request it makes."),
            ))
        }
        // Timeout(None) sets every limit to None; `read=` can set that one back.
        "httpx.Timeout" => {
            let default = positional.first().or_else(|| keywords.get("timeout"));
            let read_off = match keywords.get("read") {
                Some(read) => is_none(read),
                None => default.is_some_and(is_none),
            };
            read_off.then(|| {
                finding(
                    ctx,
                    call,
                    TIMEOUT_NONE,
                    format!(
                        "{} sets no read timeout, so a server that stops answering is waited \
                         on forever.",
                        text(call, ctx.src)
                    ),
                )
            })
        }
        // Every ClientTimeout field defaults to None. Reads are bounded only by `total`
        // (the first positional argument) or `sock_read`.
        "aiohttp.ClientTimeout" => {
            let total = positional.first().or_else(|| keywords.get("total"));
            let bounded = total.is_some_and(|t| !is_none(t))
                || keywords.get("sock_read").is_some_and(|t| !is_none(t));
            if bounded {
                return None;
            }
            Some(finding(
                ctx,
                call,
                TIMEOUT_NONE,
                format!(
                    "{} sets neither total nor sock_read, so a server that stops answering \
                     is waited on forever.",
                    text(call, ctx.src)
                ),
            ))
        }
        _ => None,
    }
}

fn finding(ctx: &Ctx, node: Node, code: &'static str, message: String) -> Finding {
    let at = node.start_position();
    Finding {
        path: ctx.path.to_string(),
        line: at.row + 1,
        col: at.column + 1,
        rule: rules::find(code).expect("every code used here is in the catalog"),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(source: &str) -> Vec<(&'static str, usize)> {
        check("t.py", source)
            .into_iter()
            .map(|f| (f.rule.code, f.line))
            .collect()
    }

    fn messages(source: &str) -> Vec<String> {
        check("t.py", source)
            .into_iter()
            .map(|f| f.message)
            .collect()
    }

    #[test]
    fn flags_a_plain_request() {
        assert_eq!(
            codes("import requests\nrequests.post(url, json=body)\n"),
            [("NH001", 2)]
        );
    }

    #[test]
    fn a_timeout_is_enough() {
        assert!(codes("import requests\nrequests.get(url, timeout=5)\n").is_empty());
        assert!(codes("import requests\nrequests.get(url, timeout=settings.T)\n").is_empty());
    }

    #[test]
    fn timeout_none_is_a_finding() {
        assert_eq!(
            codes("import requests\nrequests.get(u, timeout=None)\n"),
            [("NH002", 2)]
        );
    }

    #[test]
    fn follows_aliases_and_from_imports() {
        let src = "import requests as r\nfrom requests import post, get as fetch\n\
                   r.get(u)\npost(u)\nfetch(u)\n";
        assert_eq!(codes(src), [("NH001", 3), ("NH001", 4), ("NH001", 5)]);
    }

    #[test]
    fn kwargs_cant_be_checked() {
        assert_eq!(
            codes("import requests\nrequests.get(u, **opts)\n"),
            [("NH900", 2)]
        );
        assert!(codes("import requests\nrequests.get(u, timeout=3, **opts)\n").is_empty());
    }

    #[test]
    fn ignores_comments_strings_and_other_libraries() {
        let src = "import requests, httpx\n# requests.get(u)\ns = \"requests.get(u)\"\n\
                   httpx.get(u)\nsession.get(u)\n";
        assert!(codes(src).is_empty());
    }

    #[test]
    fn needs_the_import() {
        // A local variable that happens to be called `requests` isn't the library.
        assert!(codes("requests.get(u)\n").is_empty());
    }

    #[test]
    fn subprocess() {
        let src = "import subprocess\nfrom subprocess import check_output\n\
                   subprocess.run(['git', 'fetch'])\ncheck_output(cmd, timeout=60)\n\
                   subprocess.Popen(cmd)\n";
        assert_eq!(codes(src), [("NH003", 3)]);
    }

    #[test]
    fn calls_inside_functions_and_classes() {
        let src = "import requests\n\nclass Gateway:\n    def pay(self):\n        \
                   return requests.post(self.url, data=self.body)\n";
        assert_eq!(codes(src), [("NH001", 5)]);
    }

    #[test]
    fn follows_a_session_variable() {
        let src = "\
import requests
s = requests.Session()
s.get(u)
s.post(u, timeout=5)
s.close()
def f():
    s.put(u)
";
        assert_eq!(codes(src), [("NH001", 3), ("NH001", 7)]);
    }

    #[test]
    fn follows_with_and_inline_sessions() {
        let src = "\
from requests import Session
with Session() as s, open(p) as fh:
    s.get(u)
    fh.read()
Session().post(u)
";
        assert_eq!(codes(src), [("NH001", 3), ("NH001", 5)]);
    }

    #[test]
    fn follows_self_attributes_across_methods() {
        let src = "\
import requests
class Api:
    def __init__(self):
        self.session = requests.Session()
    def fetch(self):
        return self.session.get(self.url)
class Other:
    def fetch(self):
        return self.session.get(self.url)
";
        assert_eq!(codes(src), [("NH001", 6)]);
    }

    #[test]
    fn follows_typed_parameters() {
        let src = "\
import requests
def fetch(session: requests.Session, other, url):
    session.get(url)
    other.get(url)
";
        assert_eq!(codes(src), [("NH001", 3)]);
    }

    #[test]
    fn names_are_scoped_to_their_function() {
        // `s` in g() is a parameter, not the module's session.
        let src = "\
import requests
s = requests.Session()
def g(s):
    s.get(u)
def h():
    s = make()
    s.get(u)
";
        assert!(codes(src).is_empty());
    }

    #[test]
    fn a_name_with_two_meanings_is_left_alone() {
        let src = "import requests\nc = requests.Session()\nc = cache\nc.get(k)\n";
        assert!(codes(src).is_empty());
    }

    #[test]
    fn a_mounted_adapter_may_set_the_timeout() {
        let src = "\
import requests
s = requests.Session()
s.mount('https://', TimeoutAdapter(10))
s.get(u)
t = requests.Session()
t.request = functools.partial(t.request, timeout=5)
t.get(u)
";
        assert!(codes(src).is_empty());
    }

    #[test]
    fn session_timeout_attribute_does_nothing() {
        let src = "import requests\ns = requests.Session()\ns.timeout = 10\ns.get(u)\n";
        let got = messages(src);
        assert_eq!(got.len(), 1);
        assert!(got[0].ends_with("Setting the session's .timeout does nothing in requests."));
    }

    #[test]
    fn httpx_is_bounded_unless_turned_off() {
        let src = "\
import httpx
httpx.get(u)
httpx.get(u, timeout=None)
c = httpx.Client(timeout=None)
c.get(u)
with httpx.AsyncClient() as a:
    a.get(u)
    a.get(u, timeout=None)
";
        assert_eq!(codes(src), [("NH002", 3), ("NH002", 4), ("NH002", 8)]);
    }

    #[test]
    fn httpx_timeout_objects() {
        let src = "\
import httpx
httpx.Timeout(None)
httpx.Timeout(None, connect=5.0)
httpx.Timeout(None, read=30)
httpx.Timeout(10.0)
httpx.Timeout(10.0, read=None)
httpx.Timeout(timeout=None)
";
        assert_eq!(
            codes(src),
            [("NH002", 2), ("NH002", 3), ("NH002", 6), ("NH002", 7)]
        );
    }

    #[test]
    fn aiohttp() {
        let src = "\
import aiohttp
async def main():
    async with aiohttp.ClientSession(timeout=None) as session:
        await session.get(u)
        await session.get(u, timeout=None)
    aiohttp.ClientTimeout()
    aiohttp.ClientTimeout(total=None, sock_connect=5)
    aiohttp.ClientTimeout(total=30)
    aiohttp.ClientTimeout(sock_read=10)
    aiohttp.ClientTimeout(60)
";
        assert_eq!(codes(src), [("NH002", 5), ("NH002", 6), ("NH002", 7)]);
    }
}
