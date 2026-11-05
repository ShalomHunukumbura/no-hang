//! Small helpers over tree-sitter shared by every language.

use tree_sitter::Node;

/// The source text of a node.
pub fn text<'s>(node: Node, src: &'s [u8]) -> &'s str {
    node.utf8_text(src).unwrap_or("")
}

/// Visit every node under `node`, depth first.
pub fn walk<'t>(node: Node<'t>, visit: &mut impl FnMut(Node<'t>)) {
    visit(node);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(child, visit);
    }
}

/// The named children of a node.
pub fn named_children(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}

/// A string literal's contents without its quotes, for `'axios'` or `"net/http"`.
pub fn unquote(literal: &str) -> &str {
    literal.trim_matches(|c| c == '"' || c == '\'' || c == '`')
}
