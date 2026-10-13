//! Atlassian Document Format (ADF).
//!
//! ADF is the JSON rich-text format that the Jira and Confluence v3 REST APIs use for issue
//! descriptions, comments, and pages. The v2 APIs use plain strings for the same fields, so this
//! crate also converts between ADF and plain text.
//!
//! [`Node`] keeps every attribute it does not know about, so documents round-trip without loss.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// One ADF node. A document is a `doc` node whose `content` holds block nodes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    #[serde(rename = "type")]
    pub kind: String,
    /// Only on the root `doc` node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content: Vec<Node>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub marks: Vec<Mark>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attrs: Option<Map<String, Value>>,
}

/// Inline formatting, such as `strong`, `em`, `code`, or `link`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mark {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attrs: Option<Map<String, Value>>,
}

impl Node {
    fn new(kind: &str) -> Self {
        Self {
            kind: kind.into(),
            version: None,
            text: None,
            content: Vec::new(),
            marks: Vec::new(),
            attrs: None,
        }
    }

    /// An empty document.
    pub fn doc(content: Vec<Node>) -> Self {
        Self {
            version: Some(1),
            content,
            ..Self::new("doc")
        }
    }

    pub fn paragraph(content: Vec<Node>) -> Self {
        Self {
            content,
            ..Self::new("paragraph")
        }
    }

    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: Some(text.into()),
            ..Self::new("text")
        }
    }

    pub fn hard_break() -> Self {
        Self::new("hardBreak")
    }

    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.as_ref()?.get(name)?.as_str()
    }

    /// True for nodes that start a new line in plain text.
    fn is_block(&self) -> bool {
        !matches!(
            self.kind.as_str(),
            "text" | "hardBreak" | "mention" | "emoji" | "date" | "status" | "inlineCard"
        )
    }
}

/// Converts plain text to an ADF document, the way the v2 API stores a plain string.
///
/// Blank lines separate paragraphs. Single newlines become hard breaks inside a paragraph.
pub fn from_plain_text(text: &str) -> Node {
    let normalized = text.replace("\r\n", "\n");
    let paragraphs = normalized
        .split("\n\n")
        .filter(|block| !block.is_empty())
        .map(|block| {
            let mut inline = Vec::new();
            for (i, line) in block.split('\n').enumerate() {
                if i > 0 {
                    inline.push(Node::hard_break());
                }
                if !line.is_empty() {
                    inline.push(Node::text(line));
                }
            }
            Node::paragraph(inline)
        })
        .collect();
    Node::doc(paragraphs)
}

/// Converts an ADF document to plain text. This is the inverse of [`from_plain_text`] for
/// documents that it produced, and a readable approximation for richer documents.
pub fn to_plain_text(node: &Node) -> String {
    let mut blocks = Vec::new();
    collect_blocks(node, &mut blocks);
    blocks.join("\n\n")
}

/// Walks block nodes and pushes the text of each leaf block (paragraph, heading, code block).
fn collect_blocks(node: &Node, blocks: &mut Vec<String>) {
    let has_block_children = node.content.iter().any(Node::is_block);
    if has_block_children {
        for child in &node.content {
            if child.is_block() {
                collect_blocks(child, blocks);
            }
        }
    } else if node.kind != "doc" {
        let mut line = String::new();
        inline_text(node, &mut line);
        blocks.push(line);
    }
}

fn inline_text(node: &Node, out: &mut String) {
    match node.kind.as_str() {
        "text" => out.push_str(node.text.as_deref().unwrap_or_default()),
        "hardBreak" => out.push('\n'),
        "mention" | "emoji" | "status" => out.push_str(
            node.attr("text")
                .or_else(|| node.attr("shortName"))
                .unwrap_or_default(),
        ),
        "inlineCard" => out.push_str(node.attr("url").unwrap_or_default()),
        _ => {
            for child in &node.content {
                inline_text(child, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_round_trips() {
        for text in [
            "",
            "one line",
            "line one\nline two",
            "para one\n\npara two\nwith break",
        ] {
            assert_eq!(to_plain_text(&from_plain_text(text)), text, "{text:?}");
        }
    }

    #[test]
    fn plain_text_serializes_as_adf() {
        let json = serde_json::to_value(from_plain_text("hi")).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "type": "doc",
                "version": 1,
                "content": [{"type": "paragraph", "content": [{"type": "text", "text": "hi"}]}]
            })
        );
    }

    #[test]
    fn rich_documents_flatten_to_readable_text() {
        let doc: Node = serde_json::from_value(serde_json::json!({
            "type": "doc", "version": 1,
            "content": [
                {"type": "heading", "attrs": {"level": 2}, "content": [{"type": "text", "text": "Steps"}]},
                {"type": "bulletList", "content": [
                    {"type": "listItem", "content": [{"type": "paragraph", "content": [
                        {"type": "text", "text": "Ping "},
                        {"type": "mention", "attrs": {"id": "abc", "text": "@Ana"}}
                    ]}]}
                ]}
            ]
        }))
        .unwrap();
        assert_eq!(to_plain_text(&doc), "Steps\n\nPing @Ana");
    }

    #[test]
    fn unknown_attributes_survive_a_round_trip() {
        let json = serde_json::json!({
            "type": "doc", "version": 1,
            "content": [{"type": "panel", "attrs": {"panelType": "info"}, "content": [
                {"type": "paragraph", "content": [{"type": "text", "text": "x", "marks": [{"type": "strong"}]}]}
            ]}]
        });
        let doc: Node = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(serde_json::to_value(doc).unwrap(), json);
    }
}
