//! Markdown to ADF and back, for editing rich text as Markdown.
//!
//! Supported: paragraphs, headings, bullet and numbered lists, code blocks, quotes, rules,
//! hard breaks, images, and the marks bold, italic, strikethrough, inline code, and links. Other ADF
//! nodes (panels, mentions, tables) keep their text when written as Markdown.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use serde_json::{Map, Value, json};

use crate::{Mark, Node};

fn attrs(pairs: &[(&str, Value)]) -> Option<Map<String, Value>> {
    Some(
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
    )
}

fn block(kind: &str) -> Node {
    Node {
        kind: kind.into(),
        version: None,
        text: None,
        content: Vec::new(),
        marks: Vec::new(),
        attrs: None,
    }
}

/// Converts Markdown to an ADF document.
pub fn from_markdown(markdown: &str) -> Node {
    let parser = Parser::new_ext(markdown, Options::ENABLE_STRIKETHROUGH);
    // Open containers; the last one receives new children.
    let mut stack: Vec<Node> = vec![Node::doc(Vec::new())];
    let mut marks: Vec<Mark> = Vec::new();

    fn push(stack: &mut [Node], node: Node) {
        stack
            .last_mut()
            .expect("doc is always open")
            .content
            .push(node);
    }
    fn close(stack: &mut Vec<Node>) {
        let node = stack.pop().expect("an open container");
        push(stack, node);
    }
    // Inline content outside a paragraph (for example in a tight list item) gets one.
    fn ensure_inline_parent(stack: &mut Vec<Node>) {
        let kind = stack.last().map(|n| n.kind.as_str()).unwrap_or_default();
        if matches!(kind, "doc" | "listItem" | "blockquote") {
            stack.push(Node::paragraph(Vec::new()));
        }
    }

    for event in parser {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => stack.push(Node::paragraph(Vec::new())),
                Tag::Heading { level, .. } => {
                    let level = match level {
                        HeadingLevel::H1 => 1,
                        HeadingLevel::H2 => 2,
                        HeadingLevel::H3 => 3,
                        HeadingLevel::H4 => 4,
                        HeadingLevel::H5 => 5,
                        HeadingLevel::H6 => 6,
                    };
                    stack.push(Node {
                        attrs: attrs(&[("level", json!(level))]),
                        ..block("heading")
                    });
                }
                Tag::BlockQuote(_) => stack.push(block("blockquote")),
                Tag::CodeBlock(kind) => {
                    let language = match kind {
                        CodeBlockKind::Fenced(lang) if !lang.is_empty() => Some(lang.to_string()),
                        _ => None,
                    };
                    let mut node = block("codeBlock");
                    if let Some(language) = language {
                        node.attrs = attrs(&[("language", json!(language))]);
                    }
                    stack.push(node);
                }
                Tag::List(Some(start)) => {
                    let mut node = block("orderedList");
                    if start != 1 {
                        node.attrs = attrs(&[("order", json!(start))]);
                    }
                    stack.push(node);
                }
                Tag::List(None) => stack.push(block("bulletList")),
                Tag::Item => stack.push(block("listItem")),
                Tag::Emphasis => marks.push(Mark {
                    kind: "em".into(),
                    attrs: None,
                }),
                Tag::Strong => marks.push(Mark {
                    kind: "strong".into(),
                    attrs: None,
                }),
                Tag::Strikethrough => marks.push(Mark {
                    kind: "strike".into(),
                    attrs: None,
                }),
                Tag::Link { dest_url, .. } => marks.push(Mark {
                    kind: "link".into(),
                    attrs: attrs(&[("href", json!(dest_url.to_string()))]),
                }),
                // The image's alt text arrives as text events until it ends.
                Tag::Image { dest_url, .. } => {
                    ensure_inline_parent(&mut stack);
                    stack.push(Node::image(&dest_url, ""));
                }
                _ => {}
            },
            Event::End(tag) => match tag {
                // A paragraph holding only an image becomes an image block.
                TagEnd::Paragraph
                    if stack.last().is_some_and(|p| {
                        p.content.len() == 1 && p.content[0].kind == "mediaInline"
                    }) =>
                {
                    let mut paragraph = stack.pop().expect("an open paragraph");
                    let image = paragraph.content.remove(0);
                    push(&mut stack, Node::image_block(image));
                }
                TagEnd::Image => close(&mut stack),
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::BlockQuote(_)
                | TagEnd::CodeBlock
                | TagEnd::List(_) => close(&mut stack),
                TagEnd::Item => {
                    // A tight list item's text sits in an implicit paragraph; close it first.
                    if stack.last().is_some_and(|n| n.kind == "paragraph") {
                        close(&mut stack);
                    }
                    close(&mut stack);
                }
                TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link => {
                    marks.pop();
                }
                _ => {}
            },
            Event::Text(text) => {
                if let Some(image) = stack.last_mut().filter(|n| n.kind == "mediaInline") {
                    let attrs = image.attrs.get_or_insert_with(Map::new);
                    let alt = attrs.get("alt").and_then(Value::as_str).unwrap_or_default();
                    let alt = format!("{alt}{text}");
                    attrs.insert("alt".into(), json!(alt));
                } else if stack.last().is_some_and(|n| n.kind == "codeBlock") {
                    let code = text.strip_suffix('\n').unwrap_or(&text);
                    push(&mut stack, Node::text(code));
                } else {
                    ensure_inline_parent(&mut stack);
                    push(
                        &mut stack,
                        Node {
                            marks: marks.clone(),
                            ..Node::text(text.to_string())
                        },
                    );
                }
            }
            Event::Code(code) => {
                ensure_inline_parent(&mut stack);
                let mut code_marks = marks.clone();
                code_marks.push(Mark {
                    kind: "code".into(),
                    attrs: None,
                });
                push(
                    &mut stack,
                    Node {
                        marks: code_marks,
                        ..Node::text(code.to_string())
                    },
                );
            }
            Event::HardBreak | Event::SoftBreak => {
                ensure_inline_parent(&mut stack);
                push(&mut stack, Node::hard_break());
            }
            Event::Rule => push(&mut stack, block("rule")),
            _ => {}
        }
    }
    // Close anything left open by unbalanced input.
    while stack.len() > 1 {
        close(&mut stack);
    }
    stack.pop().expect("doc")
}

/// Converts an ADF document to Markdown.
pub fn to_markdown(doc: &Node) -> String {
    let mut out = String::new();
    blocks(&doc.content, "", &mut out);
    out.trim_end().to_string()
}

fn blocks(nodes: &[Node], indent: &str, out: &mut String) {
    for (i, node) in nodes.iter().enumerate() {
        if i > 0 && !matches!(node.kind.as_str(), "listItem") {
            out.push('\n');
        }
        block_markdown(node, indent, out);
    }
}

fn block_markdown(node: &Node, indent: &str, out: &mut String) {
    let attr = |name: &str| node.attrs.as_ref().and_then(|a| a.get(name)).cloned();
    match node.kind.as_str() {
        "paragraph" => {
            out.push_str(indent);
            out.push_str(&inline(&node.content).replace('\n', &format!("\n{indent}")));
            out.push('\n');
        }
        "heading" => {
            let level = attr("level")
                .and_then(|v| v.as_u64())
                .unwrap_or(1)
                .clamp(1, 6) as usize;
            out.push_str(&format!(
                "{indent}{} {}\n",
                "#".repeat(level),
                inline(&node.content)
            ));
        }
        "codeBlock" => {
            let language = attr("language")
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            let code: String = node.content.iter().filter_map(|n| n.text.clone()).collect();
            out.push_str(&format!("{indent}```{language}\n"));
            for line in code.lines() {
                out.push_str(&format!("{indent}{line}\n"));
            }
            out.push_str(&format!("{indent}```\n"));
        }
        "blockquote" => {
            let mut inner = String::new();
            blocks(&node.content, "", &mut inner);
            for line in inner.trim_end().lines() {
                out.push_str(&format!("{indent}> {line}\n"));
            }
        }
        "rule" => out.push_str(&format!("{indent}---\n")),
        "mediaSingle" => {
            out.push_str(indent);
            out.push_str(&image_markdown(node));
            out.push('\n');
        }
        "bulletList" | "orderedList" => {
            let start = attr("order").and_then(|v| v.as_u64()).unwrap_or(1);
            for (i, item) in node.content.iter().enumerate() {
                let marker = if node.kind == "bulletList" {
                    "- ".to_string()
                } else {
                    format!("{}. ", start + i as u64)
                };
                let mut inner = String::new();
                blocks(&item.content, "", &mut inner);
                let pad = " ".repeat(marker.len());
                for (j, line) in inner.trim_end().lines().enumerate() {
                    if j == 0 {
                        out.push_str(&format!("{indent}{marker}{line}\n"));
                    } else if line.is_empty() {
                        out.push('\n');
                    } else {
                        out.push_str(&format!("{indent}{pad}{line}\n"));
                    }
                }
            }
        }
        // Panels, tables, and other nodes keep their text.
        _ => {
            if node
                .content
                .iter()
                .any(|c| c.kind == "text" || c.kind == "hardBreak")
            {
                out.push_str(indent);
                out.push_str(&inline(&node.content));
                out.push('\n');
            } else {
                blocks(&node.content, indent, out);
            }
        }
    }
}

fn inline(nodes: &[Node]) -> String {
    let mut out = String::new();
    for node in nodes {
        match node.kind.as_str() {
            "text" => {
                let mut text = node.text.clone().unwrap_or_default();
                let has = |kind: &str| node.marks.iter().any(|m| m.kind == kind);
                if has("code") {
                    text = format!("`{text}`");
                } else {
                    if has("strong") {
                        text = format!("**{text}**");
                    }
                    if has("em") {
                        text = format!("*{text}*");
                    }
                    if has("strike") {
                        text = format!("~~{text}~~");
                    }
                }
                if let Some(link) = node.marks.iter().find(|m| m.kind == "link") {
                    let href = link
                        .attrs
                        .as_ref()
                        .and_then(|a| a.get("href"))
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    text = format!("[{text}]({href})");
                }
                out.push_str(&text);
            }
            "hardBreak" => out.push('\n'),
            "mediaInline" | "media" => out.push_str(&image_markdown(node)),
            _ => out.push_str(&crate::to_plain_text(node)),
        }
    }
    out
}

/// `![alt](source)` for an image node.
fn image_markdown(node: &Node) -> String {
    match node.image_source() {
        Some((source, alt)) => format!("![{alt}]({})", source.text()),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(markdown: &str) {
        let doc = from_markdown(markdown);
        assert_eq!(to_markdown(&doc), markdown, "{doc:#?}");
    }

    #[test]
    fn round_trips_common_markdown() {
        round_trip("Plain paragraph.");
        round_trip("# Title\n\nSome **bold**, *italic*, ~~gone~~, and `code`.");
        round_trip("- one\n- two\n- three");
        round_trip("1. first\n2. second");
        round_trip("```rust\nfn main() {}\n```");
        round_trip("> quoted");
        round_trip("A [link](https://example.com) here.");
        round_trip("Before\n\n---\n\nAfter");
        round_trip("![Diagram](arch.png)");
        round_trip("See ![logo](https://example.com/logo.png) here.");
    }

    #[test]
    fn images_become_media_nodes() {
        let doc = from_markdown("![Diagram](arch.png)\n\nText ![x](https://e.com/x.png)");
        assert_eq!(doc.content[0].kind, "mediaSingle");
        assert_eq!(doc.content[0].content[0].kind, "media");
        let inline = &doc.content[1].content[1];
        assert_eq!(inline.kind, "mediaInline");
        assert_eq!(
            inline.image_source(),
            Some((crate::ImageSource::Url("https://e.com/x.png"), "x"))
        );
    }

    #[test]
    fn builds_adf_nodes() {
        let doc = from_markdown("## Heading\n\n- item with **bold**");
        let json = serde_json::to_value(&doc).unwrap();
        assert_eq!(json["content"][0]["type"], "heading");
        assert_eq!(json["content"][0]["attrs"]["level"], 2);
        let item = &json["content"][1]["content"][0];
        assert_eq!(item["type"], "listItem");
        assert_eq!(item["content"][0]["type"], "paragraph");
        assert_eq!(
            item["content"][0]["content"][1]["marks"][0]["type"],
            "strong"
        );
    }

    #[test]
    fn plain_text_still_reads_well() {
        let doc = from_markdown("Line one\nline two");
        assert_eq!(crate::to_plain_text(&doc), "Line one\nline two");
    }
}
