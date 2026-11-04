//! Python: find calls into known libraries and check how their timeout is set.
//!
//! The file is parsed with tree-sitter, so comments, strings and docstrings that mention
//! `requests.get(` are never mistaken for calls, and `import requests as r` or
//! `from requests import post` are followed to the real function.

use std::collections::HashMap;

use tree_sitter::{Node, Parser};

use crate::finding::Finding;
use crate::rules::{self, CANT_CHECK, HTTP_NO_TIMEOUT, SUBPROCESS_NO_TIMEOUT, TIMEOUT_NONE};

/// A library function that blocks, and the keyword argument that bounds it.
struct Target {
    module: &'static str,
    functions: &'static [&'static str],
    param: &'static str,
    rule: &'static str,
}

/// What each library does when no timeout is passed is the knowledge this table holds.
/// Only functions that wait forever by default are listed: httpx (5 seconds) and aiohttp
/// (5 minutes) are bounded unless told otherwise, so their plain calls are fine.
const TARGETS: &[Target] = &[
    Target {
        module: "requests",
        functions: &[
            "get", "post", "put", "patch", "delete", "head", "options", "request",
        ],
        param: "timeout",
        rule: HTTP_NO_TIMEOUT,
    },
    Target {
        module: "subprocess",
        functions: &["run", "call", "check_call", "check_output"],
        param: "timeout",
        rule: SUBPROCESS_NO_TIMEOUT,
    },
];

/// Names a file binds by importing.
#[derive(Default, Debug)]
struct Imports {
    /// `import requests as r` binds r -> requests.
    modules: HashMap<String, String>,
    /// `from requests import post as p` binds p -> requests.post.
    names: HashMap<String, String>,
}

pub fn check(path: &str, source: &str) -> Vec<Finding> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .expect("the Python grammar matches the tree-sitter version");
    let Some(tree) = parser.parse(source, None) else {
        return Vec::new();
    };
    let src = source.as_bytes();
    let root = tree.root_node();

    let mut imports = Imports::default();
    walk(root, &mut |node| collect_import(node, src, &mut imports));

    let mut findings = Vec::new();
    walk(root, &mut |node| {
        if node.kind() == "call" {
            if let Some(f) = check_call(node, src, &imports, path) {
                findings.push(f);
            }
        }
    });
    findings
}

/// Visit every node, depth first.
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

/// The dotted name a call's function refers to, with imports resolved:
/// `r.post` after `import requests as r` is `requests.post`.
fn resolve(function: Node, src: &[u8], imports: &Imports) -> Option<String> {
    match function.kind() {
        "identifier" => imports.names.get(text(function, src)).cloned(),
        "attribute" => {
            let mut parts = vec![text(function.child_by_field_name("attribute")?, src)];
            let mut object = function.child_by_field_name("object")?;
            while object.kind() == "attribute" {
                parts.push(text(object.child_by_field_name("attribute")?, src));
                object = object.child_by_field_name("object")?;
            }
            // Anything else (a call, a subscript) is a value made at runtime: milestone 2.
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

fn target_for(qualified: &str) -> Option<&'static Target> {
    let (module, function) = qualified.rsplit_once('.')?;
    TARGETS
        .iter()
        .find(|t| t.module == module && t.functions.contains(&function))
}

fn check_call(call: Node, src: &[u8], imports: &Imports, path: &str) -> Option<Finding> {
    let function = call.child_by_field_name("function")?;
    let qualified = resolve(function, src, imports)?;
    let target = target_for(&qualified)?;
    let arguments = call.child_by_field_name("arguments")?;
    let shown = text(function, src);

    let mut splat = None;
    let mut cursor = arguments.walk();
    for arg in arguments.named_children(&mut cursor) {
        match arg.kind() {
            "keyword_argument" => {
                let name = arg.child_by_field_name("name").map(|n| text(n, src));
                if name == Some(target.param) {
                    let value = arg.child_by_field_name("value")?;
                    if value.kind() == "none" {
                        return Some(finding(
                            path,
                            value,
                            TIMEOUT_NONE,
                            format!("{shown}({}=None) turns the timeout off.", target.param),
                        ));
                    }
                    return None;
                }
            }
            "dictionary_splat" => splat = Some(arg),
            _ => {}
        }
    }

    if let Some(splat) = splat {
        return Some(finding(
            path,
            splat,
            CANT_CHECK,
            format!(
                "{shown}({}) may set {} inside the dictionary, so it can't be checked.",
                text(splat, src),
                target.param
            ),
        ));
    }

    let message = match target.rule {
        SUBPROCESS_NO_TIMEOUT => {
            format!("{shown}() has no timeout, so a command that never exits blocks forever.")
        }
        _ => {
            format!("{shown}() has no timeout, so it waits forever if the server stops answering.")
        }
    };
    Some(finding(path, call, target.rule, message))
}

fn finding(path: &str, node: Node, code: &'static str, message: String) -> Finding {
    let at = node.start_position();
    Finding {
        path: path.to_string(),
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
}
