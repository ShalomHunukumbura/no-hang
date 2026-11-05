//! Go: net/http, os/exec and database/sql.
//!
//! - `http.Get`, `Post`, `PostForm` and `Head` use `http.DefaultClient`, whose `Timeout` is
//!   0, meaning none. There's no way to give them a context, so they're always findings.
//! - `http.Client{}` with no `Timeout` is the same, unless the requests it sends carry a
//!   context deadline (a file that builds requests with `http.NewRequestWithContext` is
//!   taken to do that) or its inline Transport sets `ResponseHeaderTimeout`.
//! - `exec.Command` has no deadline; `exec.CommandContext` with a timeout context does.
//! - `database/sql`'s `Query`, `Exec` and friends have no deadline; their `...Context`
//!   versions take one.

use std::collections::HashMap;

use tree_sitter::{Node, Parser};

use crate::finding::Finding;
use crate::rules::{DB_NO_TIMEOUT, HTTP_NO_TIMEOUT, SUBPROCESS_NO_TIMEOUT, TIMEOUT_NONE};
use crate::syntax::{named_children, text, unquote, walk};

const MODULES: &[&str] = &["net/http", "os/exec", "database/sql"];

/// `db.Query(...)` and the rest, which have a `...Context` version.
const SQL_METHODS: &[&str] = &["Query", "QueryRow", "Exec", "Prepare", "Begin", "Ping"];

struct Ctx<'a> {
    src: &'a [u8],
    path: &'a str,
    /// Local package name -> import path: `http` -> `net/http`.
    imports: HashMap<String, String>,
    /// Every value each name is given: true when it's `sql.Open(...)`.
    sql_values: HashMap<String, Vec<bool>>,
    /// The file builds requests with a context.
    request_context: bool,
}

pub fn check(path: &str, source: &str) -> Vec<Finding> {
    if !MODULES.iter().any(|m| source.contains(m)) {
        return Vec::new();
    }
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_go::LANGUAGE.into())
        .expect("the grammar matches the tree-sitter version");
    let Some(tree) = parser.parse(source, None) else {
        return Vec::new();
    };
    let mut ctx = Ctx {
        src: source.as_bytes(),
        path,
        imports: HashMap::new(),
        sql_values: HashMap::new(),
        request_context: false,
    };
    let mut nodes = Vec::new();
    walk(tree.root_node(), &mut |n| nodes.push(n));

    for node in &nodes {
        if node.kind() == "import_spec" {
            collect_import(*node, &mut ctx);
        }
    }
    for node in &nodes {
        collect_binding(*node, &mut ctx);
    }
    nodes
        .iter()
        .filter_map(|n| match n.kind() {
            "call_expression" => check_call(*n, &ctx),
            "composite_literal" => check_client(*n, &ctx),
            _ => None,
        })
        .collect()
}

fn collect_import(spec: Node, ctx: &mut Ctx) {
    let Some(path) = spec.child_by_field_name("path") else {
        return;
    };
    let path = unquote(text(path, ctx.src)).to_string();
    let local = match spec.child_by_field_name("name") {
        Some(name) if name.kind() == "package_identifier" => text(name, ctx.src).to_string(),
        // `_` and `.` imports don't give a name to call through.
        Some(_) => return,
        None => path.rsplit('/').next().unwrap_or(&path).to_string(),
    };
    ctx.imports.insert(local, path);
}

/// `http.Get` is `net/http.Get`; `http.DefaultClient.Do` is `net/http.DefaultClient.Do`.
fn resolve(expr: Node, ctx: &Ctx) -> Option<String> {
    match expr.kind() {
        "identifier" | "package_identifier" => ctx.imports.get(text(expr, ctx.src)).cloned(),
        "selector_expression" => {
            let operand = resolve(expr.child_by_field_name("operand")?, ctx)?;
            let field = text(expr.child_by_field_name("field")?, ctx.src);
            Some(format!("{operand}.{field}"))
        }
        "qualified_type" => {
            let package = ctx
                .imports
                .get(text(expr.child_by_field_name("package")?, ctx.src))?;
            let name = text(expr.child_by_field_name("name")?, ctx.src);
            Some(format!("{package}.{name}"))
        }
        _ => None,
    }
}

fn is_sql_open(value: Node, ctx: &Ctx) -> bool {
    value.kind() == "call_expression"
        && value
            .child_by_field_name("function")
            .and_then(|f| resolve(f, ctx))
            .is_some_and(|q| matches!(q.as_str(), "database/sql.Open" | "database/sql.OpenDB"))
}

fn collect_binding(node: Node, ctx: &mut Ctx) {
    // `db, err := sql.Open(...)`, `db, err = sql.Open(...)` or `var db, err = sql.Open(...)`.
    let (names, values) = match node.kind() {
        "short_var_declaration" | "assignment_statement" => (
            node.child_by_field_name("left").map(named_children),
            node.child_by_field_name("right").map(named_children),
        ),
        "var_spec" => {
            let mut cursor = node.walk();
            let names: Vec<Node> = node.children_by_field_name("name", &mut cursor).collect();
            (
                Some(names),
                node.child_by_field_name("value").map(named_children),
            )
        }
        "call_expression" => {
            let function = node.child_by_field_name("function");
            if function.and_then(|f| resolve(f, ctx)).as_deref()
                == Some("net/http.NewRequestWithContext")
            {
                ctx.request_context = true;
            }
            return;
        }
        _ => return,
    };
    let (Some(names), Some(values)) = (names, values) else {
        return;
    };
    // `db, err := sql.Open(...)`: one call, two names; the first is the handle.
    let opened = values.len() == 1 && is_sql_open(values[0], ctx);
    for (i, name) in names.iter().enumerate() {
        if name.kind() != "identifier" {
            continue;
        }
        let is_db = if opened {
            i == 0
        } else {
            values.get(i).is_some_and(|v| is_sql_open(*v, ctx))
        };
        ctx.sql_values
            .entry(text(*name, ctx.src).to_string())
            .or_default()
            .push(is_db);
    }
}

fn check_call(call: Node, ctx: &Ctx) -> Option<Finding> {
    let function = call.child_by_field_name("function")?;
    let shown = text(function, ctx.src);
    let finding = |code, message| Some(Finding::at(ctx.path, call, code, message));

    if let Some(qualified) = resolve(function, ctx) {
        return match qualified.as_str() {
            "net/http.Get" | "net/http.Post" | "net/http.PostForm" | "net/http.Head" => finding(
                HTTP_NO_TIMEOUT,
                format!(
                    "{shown}() uses http.DefaultClient, which has no timeout, so it waits \
                     forever if the server stops answering."
                ),
            ),
            q if q.starts_with("net/http.DefaultClient.") => finding(
                HTTP_NO_TIMEOUT,
                format!(
                    "{shown}() uses http.DefaultClient, which has no timeout, so it waits \
                     forever if the server stops answering."
                ),
            ),
            "os/exec.Command" => finding(
                SUBPROCESS_NO_TIMEOUT,
                format!("{shown}() has no deadline, so a command that never exits blocks forever."),
            ),
            _ => None,
        };
    }

    // `db.Query(...)` on a handle from sql.Open.
    if function.kind() != "selector_expression" {
        return None;
    }
    let operand = function.child_by_field_name("operand")?;
    let method = text(function.child_by_field_name("field")?, ctx.src);
    if operand.kind() != "identifier" || !SQL_METHODS.contains(&method) {
        return None;
    }
    let values = ctx.sql_values.get(text(operand, ctx.src))?;
    if !values.iter().all(|v| *v) {
        return None;
    }
    finding(
        DB_NO_TIMEOUT,
        format!(
            "{shown}() has no context, so a query can run forever. {method}Context takes one \
             with a deadline."
        ),
    )
}

/// `&http.Client{...}` with no Timeout.
fn check_client(literal: Node, ctx: &Ctx) -> Option<Finding> {
    let typ = literal.child_by_field_name("type")?;
    if resolve(typ, ctx).as_deref() != Some("net/http.Client") {
        return None;
    }
    let body = literal.child_by_field_name("body")?;
    let mut timeout = None;
    for element in named_children(body) {
        if element.kind() != "keyed_element" {
            // `http.Client{t, nil, nil, 0}` sets every field by position.
            return None;
        }
        let key = element.child_by_field_name("key").map(|k| text(k, ctx.src));
        if key == Some("Timeout") {
            timeout = element.child_by_field_name("value");
        }
        // `Transport: &http.Transport{ResponseHeaderTimeout: ...}` bounds the wait for an
        // answer, which is the hang this rule is about.
        if key == Some("Transport")
            && element
                .child_by_field_name("value")
                .is_some_and(|v| text(v, ctx.src).contains("ResponseHeaderTimeout"))
        {
            return None;
        }
    }
    let shown = text(typ, ctx.src);
    match timeout {
        Some(value) => {
            let v = text(value, ctx.src);
            (v == "0").then(|| {
                Finding::at(
                    ctx.path,
                    value,
                    TIMEOUT_NONE,
                    format!("{shown}{{Timeout: 0}} turns the timeout off."),
                )
            })
        }
        None if ctx.request_context => None,
        None => Some(Finding::at(
            ctx.path,
            literal,
            HTTP_NO_TIMEOUT,
            format!(
                "{shown}{{}} has no Timeout, so a request without a context deadline waits \
                 forever if the server stops answering."
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(source: &str) -> Vec<(&'static str, usize)> {
        check("t.go", source)
            .into_iter()
            .map(|f| (f.rule.code, f.line))
            .collect()
    }

    #[test]
    fn default_client() {
        let src = "\
package main
import \"net/http\"
func main() {
	http.Get(u)
	http.DefaultClient.Do(req)
	http.NewRequest(\"GET\", u, nil)
}
";
        assert_eq!(codes(src), [("NH001", 4), ("NH001", 5)]);
    }

    #[test]
    fn client_literals() {
        let src = "\
package main
import (
	\"time\"
	nethttp \"net/http\"
)
var a = &nethttp.Client{}
var b = &nethttp.Client{Timeout: 10 * time.Second}
var c = nethttp.Client{Transport: t, Timeout: 0}
var d = &nethttp.Client{Transport: &nethttp.Transport{ResponseHeaderTimeout: 5 * time.Second}}
";
        assert_eq!(codes(src), [("NH001", 6), ("NH002", 8)]);
    }

    #[test]
    fn requests_with_a_context_are_bounded_there() {
        let src = "\
package main
import \"net/http\"
var c = &http.Client{}
func get(ctx context.Context) {
	req, _ := http.NewRequestWithContext(ctx, \"GET\", u, nil)
	c.Do(req)
}
";
        assert!(codes(src).is_empty());
    }

    #[test]
    fn exec_command() {
        let src = "\
package main
import \"os/exec\"
func main() {
	exec.Command(\"git\", \"fetch\").Run()
	exec.CommandContext(ctx, \"git\", \"fetch\").Run()
}
";
        assert_eq!(codes(src), [("NH003", 4)]);
    }

    #[test]
    fn database_sql() {
        let src = "\
package main
import \"database/sql\"
func main() {
	db, err := sql.Open(\"postgres\", dsn)
	db.Query(\"select 1\")
	db.QueryContext(ctx, \"select 1\")
	rows.Close()
	other.Exec(\"x\")
}
";
        assert_eq!(codes(src), [("NH006", 5)]);
    }

    #[test]
    fn a_name_with_two_meanings_is_left_alone() {
        let src = "\
package main
import \"database/sql\"
func a() { db, _ := sql.Open(\"pg\", dsn); db.Exec(\"x\") }
func b() { db := cache(); db.Exec(\"y\") }
";
        assert!(codes(src).is_empty());
    }
}
