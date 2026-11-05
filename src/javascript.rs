//! JavaScript and TypeScript: axios, child_process, node-postgres and ioredis.
//!
//! Each library's default was read in its source:
//!
//! - axios: `timeout: 0`, and 0 means no timeout. `axios.defaults.timeout = n` in the file
//!   bounds every call through it.
//! - child_process: `timeout` is unset (or 0) by default, so exec and the sync calls wait for
//!   the command however long it takes. spawn and fork stream output and are left alone.
//! - pg: with no `connectionTimeoutMillis`, a Pool that's full queues the caller forever,
//!   and a Client connects with no limit.
//! - ioredis 6: `connectTimeout` is 10 seconds, but `commandTimeout` has no default.
//! - fetch isn't checked: Node's fetch (undici) has 300 second header and body timeouts.

use std::collections::HashMap;

use tree_sitter::{Language, Node, Parser};

use crate::finding::Finding;
use crate::rules::{
    CANT_CHECK, DB_NO_TIMEOUT, HTTP_NO_TIMEOUT, REDIS_NO_TIMEOUT, SUBPROCESS_NO_TIMEOUT,
    TIMEOUT_NONE,
};
use crate::syntax::{named_children, text, unquote, walk};

#[derive(Clone, Copy)]
pub enum Dialect {
    JavaScript,
    TypeScript,
    Tsx,
}

/// Where a call's options object is.
#[derive(Clone, Copy)]
enum Options {
    At(usize),
    /// `execFile(file, args, options)`, where args can be left out.
    AtOrBefore(usize),
    /// `axios(config)` or `axios(url, config)`.
    ConfigOrSecond,
    /// The first object literal anywhere: `new Redis(port, host, options)`.
    AnyObject,
}

struct Function {
    names: &'static [&'static str],
    options: Options,
    key: &'static str,
    rule: &'static str,
    consequence: &'static str,
    /// `axios.defaults.timeout = n` in the file bounds it.
    axios_default: bool,
}

const SERVER_HANGS: &str = "it waits forever if the server stops answering";

const FUNCTIONS: &[Function] = &[
    Function {
        names: &["axios", "axios.request"],
        options: Options::ConfigOrSecond,
        key: "timeout",
        rule: HTTP_NO_TIMEOUT,
        consequence: SERVER_HANGS,
        axios_default: true,
    },
    Function {
        names: &["axios.get", "axios.delete", "axios.head", "axios.options"],
        options: Options::At(1),
        key: "timeout",
        rule: HTTP_NO_TIMEOUT,
        consequence: SERVER_HANGS,
        axios_default: true,
    },
    Function {
        names: &[
            "axios.post",
            "axios.put",
            "axios.patch",
            "axios.postForm",
            "axios.putForm",
            "axios.patchForm",
        ],
        options: Options::At(2),
        key: "timeout",
        rule: HTTP_NO_TIMEOUT,
        consequence: SERVER_HANGS,
        axios_default: true,
    },
    Function {
        names: &["axios.create"],
        options: Options::At(0),
        key: "timeout",
        rule: HTTP_NO_TIMEOUT,
        consequence: "every request through this instance waits forever if the server stops \
                      answering",
        axios_default: true,
    },
    Function {
        names: &["child_process.execSync", "child_process.exec"],
        options: Options::At(1),
        key: "timeout",
        rule: SUBPROCESS_NO_TIMEOUT,
        consequence: "a command that never exits blocks forever",
        axios_default: false,
    },
    Function {
        names: &[
            "child_process.execFileSync",
            "child_process.execFile",
            "child_process.spawnSync",
        ],
        options: Options::AtOrBefore(2),
        key: "timeout",
        rule: SUBPROCESS_NO_TIMEOUT,
        consequence: "a command that never exits blocks forever",
        axios_default: false,
    },
    Function {
        names: &["pg.Pool", "pg-pool"],
        options: Options::At(0),
        key: "connectionTimeoutMillis",
        rule: DB_NO_TIMEOUT,
        consequence: "once every connection is busy, the next query waits forever for one",
        axios_default: false,
    },
    Function {
        names: &["pg.Client"],
        options: Options::At(0),
        key: "connectionTimeoutMillis",
        rule: DB_NO_TIMEOUT,
        consequence: "connecting waits forever if the database doesn't answer",
        axios_default: false,
    },
    Function {
        names: &["ioredis", "ioredis.Redis", "ioredis.default"],
        options: Options::AnyObject,
        key: "commandTimeout",
        rule: REDIS_NO_TIMEOUT,
        consequence: "a command waits forever if Redis stops answering",
        axios_default: false,
    },
];

/// A file that mentions none of these can't have a finding, so it isn't parsed.
const MODULES: &[&str] = &["axios", "child_process", "pg", "ioredis"];

#[derive(Default)]
struct Imports {
    /// `import axios from 'axios'`, `import * as cp from 'child_process'` or
    /// `const pg = require('pg')` bind a name to the module.
    modules: HashMap<String, String>,
    /// `import { execSync } from 'child_process'` binds execSync -> child_process.execSync.
    names: HashMap<String, String>,
}

struct Ctx<'a> {
    src: &'a [u8],
    path: &'a str,
    imports: Imports,
    /// Some `x.defaults.timeout = n` is assigned in the file.
    axios_default: bool,
}

pub fn check(path: &str, source: &str, dialect: Dialect) -> Vec<Finding> {
    if !MODULES.iter().any(|m| source.contains(m)) {
        return Vec::new();
    }
    let language: Language = match dialect {
        Dialect::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
        Dialect::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        Dialect::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
    };
    let mut parser = Parser::new();
    parser
        .set_language(&language)
        .expect("the grammar matches the tree-sitter version");
    let Some(tree) = parser.parse(source, None) else {
        return Vec::new();
    };
    let root = tree.root_node();
    let mut ctx = Ctx {
        src: source.as_bytes(),
        path,
        imports: Imports::default(),
        axios_default: false,
    };

    let mut nodes = Vec::new();
    walk(root, &mut |n| nodes.push(n));
    for node in &nodes {
        collect(*node, &mut ctx);
    }
    nodes
        .iter()
        .filter(|n| matches!(n.kind(), "call_expression" | "new_expression"))
        .filter_map(|n| check_call(*n, &ctx))
        .collect()
}

fn module_name(literal: Node, src: &[u8]) -> String {
    let name = unquote(text(literal, src));
    name.strip_prefix("node:").unwrap_or(name).to_string()
}

/// `require('x')`, giving the module.
fn required(expr: Node, src: &[u8]) -> Option<String> {
    let expr = unwrap(expr);
    if expr.kind() != "call_expression" {
        return None;
    }
    let function = expr.child_by_field_name("function")?;
    if text(function, src) != "require" {
        return None;
    }
    let first = named_children(expr.child_by_field_name("arguments")?)
        .into_iter()
        .next()?;
    (first.kind() == "string").then(|| module_name(first, src))
}

fn collect(node: Node, ctx: &mut Ctx) {
    let src = ctx.src;
    match node.kind() {
        "import_statement" => {
            let Some(source) = node.child_by_field_name("source") else {
                return;
            };
            let module = module_name(source, src);
            let Some(clause) = named_children(node)
                .into_iter()
                .find(|c| c.kind() == "import_clause")
            else {
                return;
            };
            for part in named_children(clause) {
                match part.kind() {
                    "identifier" => {
                        ctx.imports
                            .modules
                            .insert(text(part, src).into(), module.clone());
                    }
                    "namespace_import" => {
                        if let Some(name) = part.named_child(0) {
                            ctx.imports
                                .modules
                                .insert(text(name, src).into(), module.clone());
                        }
                    }
                    "named_imports" => {
                        for spec in named_children(part) {
                            let Some(name) = spec.child_by_field_name("name") else {
                                continue;
                            };
                            let local = spec.child_by_field_name("alias").unwrap_or(name);
                            ctx.imports.names.insert(
                                text(local, src).into(),
                                format!("{module}.{}", unquote(text(name, src))),
                            );
                        }
                    }
                    _ => {}
                }
            }
        }
        "variable_declarator" => {
            let (Some(name), Some(value)) = (
                node.child_by_field_name("name"),
                node.child_by_field_name("value"),
            ) else {
                return;
            };
            let Some(module) = required(value, src) else {
                return;
            };
            match name.kind() {
                "identifier" => {
                    ctx.imports.modules.insert(text(name, src).into(), module);
                }
                // `const { execSync, exec: run } = require('child_process')`
                "object_pattern" => {
                    for part in named_children(name) {
                        let (export, local) = match part.kind() {
                            "shorthand_property_identifier_pattern" => (part, part),
                            "pair_pattern" => match (
                                part.child_by_field_name("key"),
                                part.child_by_field_name("value"),
                            ) {
                                (Some(k), Some(v)) if v.kind() == "identifier" => (k, v),
                                _ => continue,
                            },
                            _ => continue,
                        };
                        ctx.imports.names.insert(
                            text(local, src).into(),
                            format!("{module}.{}", text(export, src)),
                        );
                    }
                }
                _ => {}
            }
        }
        "assignment_expression" => {
            // `axios.defaults.timeout = 10000` or `api.defaults.timeout = 5000`.
            let Some(left) = node.child_by_field_name("left") else {
                return;
            };
            let t = text(left, src);
            if t.ends_with(".defaults.timeout") {
                ctx.axios_default = true;
            }
        }
        _ => {}
    }
}

/// Strip what doesn't change a value: parentheses, `as T`, `satisfies T`, `x!`.
fn unwrap(mut node: Node) -> Node {
    while matches!(
        node.kind(),
        "parenthesized_expression"
            | "as_expression"
            | "satisfies_expression"
            | "non_null_expression"
    ) {
        match node.named_child(0) {
            Some(inner) => node = inner,
            None => break,
        }
    }
    node
}

/// The module path a callee refers to: `cp.execSync` is `child_process.execSync`.
fn resolve(expr: Node, ctx: &Ctx) -> Option<String> {
    let expr = unwrap(expr);
    match expr.kind() {
        "identifier" => {
            let name = text(expr, ctx.src);
            ctx.imports
                .modules
                .get(name)
                .or_else(|| ctx.imports.names.get(name))
                .cloned()
        }
        "member_expression" => {
            let object = resolve(expr.child_by_field_name("object")?, ctx)?;
            let property = expr.child_by_field_name("property")?;
            Some(format!("{object}.{}", text(property, ctx.src)))
        }
        _ => None,
    }
}

/// What a call passed for its options.
enum Given<'t> {
    Object(Node<'t>),
    Unknown(Node<'t>),
    Absent,
}

fn options_argument<'t>(arguments: &[Node<'t>], options: Options) -> Given<'t> {
    let classify = |n: Node<'t>| {
        let n = unwrap(n);
        match n.kind() {
            "object" => Given::Object(n),
            // A callback, or an explicit nothing, isn't options.
            "arrow_function" | "function_expression" | "function" | "undefined" | "null" => {
                Given::Absent
            }
            _ => Given::Unknown(n),
        }
    };
    if let Some(spread) = arguments.iter().find(|a| a.kind() == "spread_element") {
        return Given::Unknown(*spread);
    }
    match options {
        Options::At(i) => arguments.get(i).map_or(Given::Absent, |a| classify(*a)),
        Options::AtOrBefore(i) => match arguments.get(i) {
            Some(a) => classify(*a),
            None => match arguments.get(i - 1).map(|a| unwrap(*a)) {
                Some(a) if a.kind() == "object" => Given::Object(a),
                _ => Given::Absent,
            },
        },
        Options::ConfigOrSecond => match arguments.first().map(|a| unwrap(*a)) {
            Some(first) if first.kind() == "object" => Given::Object(first),
            Some(first) if matches!(first.kind(), "string" | "template_string") => {
                arguments.get(1).map_or(Given::Absent, |a| classify(*a))
            }
            Some(first) => Given::Unknown(first),
            None => Given::Absent,
        },
        Options::AnyObject => {
            for a in arguments {
                let a = unwrap(*a);
                match a.kind() {
                    "object" => return Given::Object(a),
                    "string" | "template_string" | "number" => {}
                    _ => return Given::Unknown(a),
                }
            }
            Given::Absent
        }
    }
}

/// What an options object says about one key.
enum Setting<'t> {
    Value(Node<'t>),
    /// `{ timeout }` or `{ [k]: v }`: set, to something not known here.
    Set,
    /// `{ ...defaults }` with no key of its own: it may be in there.
    MaybeSpread(Node<'t>),
    Missing,
}

fn setting<'t>(object: Node<'t>, key: &str, src: &[u8]) -> Setting<'t> {
    let mut spread = None;
    for part in named_children(object) {
        match part.kind() {
            "pair" => {
                let Some(k) = part.child_by_field_name("key") else {
                    continue;
                };
                if k.kind() == "computed_property_name" {
                    return Setting::Set;
                }
                if unquote(text(k, src)) == key {
                    return match part.child_by_field_name("value") {
                        Some(v) => Setting::Value(unwrap(v)),
                        None => Setting::Set,
                    };
                }
            }
            "shorthand_property_identifier" if text(part, src) == key => return Setting::Set,
            "spread_element" => spread = Some(part),
            _ => {}
        }
    }
    spread.map_or(Setting::Missing, Setting::MaybeSpread)
}

fn check_call(call: Node, ctx: &Ctx) -> Option<Finding> {
    let is_new = call.kind() == "new_expression";
    let callee = call.child_by_field_name(if is_new { "constructor" } else { "function" })?;
    let qualified = resolve(callee, ctx)?;
    let f = FUNCTIONS
        .iter()
        .find(|f| f.names.contains(&qualified.as_str()))?;
    let shown = if is_new {
        format!("new {}", text(callee, ctx.src))
    } else {
        text(callee, ctx.src).to_string()
    };
    let arguments = call
        .child_by_field_name("arguments")
        .map(named_children)
        .unwrap_or_default();
    let key = f.key;
    let missing = || {
        if f.axios_default && ctx.axios_default {
            return None;
        }
        found(
            ctx,
            call,
            f.rule,
            format!("{shown}() has no {key}, so {}.", f.consequence),
        )
    };
    let cant_check = |node: Node| {
        found(
            ctx,
            node,
            CANT_CHECK,
            format!(
                "{shown}() gets its options from {}, which may set {key}, so it can't be checked.",
                text(node, ctx.src)
            ),
        )
    };

    match options_argument(&arguments, f.options) {
        Given::Absent => missing(),
        Given::Unknown(node) => cant_check(node),
        Given::Object(object) => match setting(object, key, ctx.src) {
            Setting::Value(v) if v.kind() == "number" && text(v, ctx.src) == "0" => found(
                ctx,
                v,
                TIMEOUT_NONE,
                format!("{shown}({{ {key}: 0 }}) turns the timeout off."),
            ),
            Setting::Value(v) if matches!(v.kind(), "null" | "undefined") => found(
                ctx,
                v,
                TIMEOUT_NONE,
                format!(
                    "{shown}({{ {key}: {} }}) turns the timeout off.",
                    text(v, ctx.src)
                ),
            ),
            Setting::Value(_) | Setting::Set => None,
            Setting::MaybeSpread(spread) => cant_check(spread),
            Setting::Missing => missing(),
        },
    }
}

fn found(ctx: &Ctx, node: Node, code: &'static str, message: String) -> Option<Finding> {
    Some(Finding::at(ctx.path, node, code, message))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes_in(source: &str, dialect: Dialect) -> Vec<(&'static str, usize)> {
        check("t", source, dialect)
            .into_iter()
            .map(|f| (f.rule.code, f.line))
            .collect()
    }

    fn codes(source: &str) -> Vec<(&'static str, usize)> {
        codes_in(source, Dialect::JavaScript)
    }

    #[test]
    fn axios_calls() {
        let src = "\
import axios from 'axios';
axios.get(url);
axios.get(url, { timeout: 5000 });
axios.post(url, body, { headers });
axios.post(url, body, { timeout: 0 });
axios({ url, method: 'post' });
axios('/x', { timeout: 10 });
axios.get(url, config);
axios.get(url, { ...defaults });
fetch(url);
";
        assert_eq!(
            codes(src),
            [
                ("NH001", 2),
                ("NH001", 4),
                ("NH002", 5),
                ("NH001", 6),
                ("NH900", 8),
                ("NH900", 9)
            ]
        );
    }

    #[test]
    fn axios_create_and_defaults() {
        let src = "const axios = require('axios');\nconst api = axios.create({ baseURL: b });\n";
        assert_eq!(codes(src), [("NH001", 2)]);
        let src = "\
const axios = require('axios');
const api = axios.create({ baseURL: b });
api.defaults.timeout = 5000;
axios.get(url);
";
        assert!(codes(src).is_empty());
    }

    #[test]
    fn child_process() {
        let src = "\
import { execSync, execFile as run } from 'node:child_process';
const cp = require('child_process');
execSync('make');
execSync('make', { timeout: 60000 });
run('git', ['fetch'], (err) => {});
run('git', { timeout: 5 });
cp.spawnSync('ls', ['-l'], { stdio: 'inherit' });
cp.spawn('server');
";
        assert_eq!(codes(src), [("NH003", 3), ("NH003", 5), ("NH003", 7)]);
    }

    #[test]
    fn node_postgres() {
        let src = "\
const { Pool } = require('pg');
import pg from 'pg';
new Pool({ connectionString });
new Pool({ connectionString, connectionTimeoutMillis: 5000 });
new pg.Client(process.env.DATABASE_URL);
new pg.Pool(config);
";
        assert_eq!(codes(src), [("NH006", 3), ("NH900", 5), ("NH900", 6)]);
    }

    #[test]
    fn ioredis_in_typescript() {
        let src = "\
import Redis from 'ioredis';
const a: Redis = new Redis();
const b = new Redis(6379, 'cache', { commandTimeout: 2000 } as RedisOptions);
const c = new Redis({ host: 'cache' });
";
        assert_eq!(
            codes_in(src, Dialect::TypeScript),
            [("NH005", 2), ("NH005", 4)]
        );
    }

    #[test]
    fn tsx_parses() {
        let src =
            "import axios from 'axios';\nconst A = () => <b onClick={() => axios.get(u)} />;\n";
        assert_eq!(codes_in(src, Dialect::Tsx), [("NH001", 2)]);
    }

    #[test]
    fn needs_the_import() {
        assert!(codes("axios.get(url);\nexecSync('x');\n").is_empty());
    }
}
