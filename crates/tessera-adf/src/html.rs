//! Conversion between ADF and Confluence's storage format, the XHTML that Confluence keeps
//! page bodies in. Confluence macros appear in storage as `ac:` elements, for example
//! `<ac:structured-macro ac:name="code">`.
//!
//! Both directions cover the nodes Confluence's editor produces: paragraphs, headings, lists,
//! quotes, code blocks, rules, tables, panels, expands, task lists, mentions, emoji, status
//! lozenges, and the text marks. Anything else keeps its text.

use scraper::node::Node as HtmlNode;
use scraper::{ElementRef, Html};
use serde_json::{Map, Value, json};

use crate::{Mark, Node};

// ADF to storage.

/// Writes an ADF document as storage-format XHTML.
pub fn to_storage(doc: &Node) -> String {
    let mut out = String::new();
    for node in &doc.content {
        block(node, &mut out);
    }
    out
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn attr<'a>(node: &'a Node, name: &str) -> Option<&'a Value> {
    node.attrs.as_ref().and_then(|a| a.get(name))
}

fn attr_str<'a>(node: &'a Node, name: &str) -> &'a str {
    attr(node, name).and_then(Value::as_str).unwrap_or_default()
}

fn blocks(nodes: &[Node], out: &mut String) {
    for node in nodes {
        block(node, out);
    }
}

fn macro_start(name: &str, out: &mut String) {
    out.push_str(&format!(
        "<ac:structured-macro ac:name=\"{name}\" ac:schema-version=\"1\">"
    ));
}

fn parameter(name: &str, value: &str, out: &mut String) {
    out.push_str(&format!(
        "<ac:parameter ac:name=\"{name}\">{}</ac:parameter>",
        escape(value)
    ));
}

fn block(node: &Node, out: &mut String) {
    match node.kind.as_str() {
        "paragraph" => {
            out.push_str("<p>");
            inlines(&node.content, out);
            out.push_str("</p>");
        }
        "heading" => {
            let level = attr(node, "level")
                .and_then(Value::as_u64)
                .unwrap_or(1)
                .clamp(1, 6);
            out.push_str(&format!("<h{level}>"));
            inlines(&node.content, out);
            out.push_str(&format!("</h{level}>"));
        }
        "bulletList" => {
            out.push_str("<ul>");
            blocks(&node.content, out);
            out.push_str("</ul>");
        }
        "orderedList" => {
            match attr(node, "order").and_then(Value::as_u64) {
                Some(start) if start != 1 => out.push_str(&format!("<ol start=\"{start}\">")),
                _ => out.push_str("<ol>"),
            }
            blocks(&node.content, out);
            out.push_str("</ol>");
        }
        "listItem" => {
            out.push_str("<li>");
            blocks(&node.content, out);
            out.push_str("</li>");
        }
        "blockquote" => {
            out.push_str("<blockquote>");
            blocks(&node.content, out);
            out.push_str("</blockquote>");
        }
        "codeBlock" => {
            let code: String = node
                .content
                .iter()
                .filter_map(|n| n.text.as_deref())
                .collect();
            macro_start("code", out);
            let language = attr_str(node, "language");
            if !language.is_empty() {
                parameter("language", language, out);
            }
            // CDATA cannot contain its own end marker, so split it where it appears.
            let safe = code.replace("]]>", "]]]]><![CDATA[>");
            out.push_str(&format!(
                "<ac:plain-text-body><![CDATA[{safe}]]></ac:plain-text-body></ac:structured-macro>"
            ));
        }
        "rule" => out.push_str("<hr />"),
        "panel" => {
            let name = match attr_str(node, "panelType") {
                "note" => "note",
                "warning" | "error" => "warning",
                "success" => "tip",
                _ => "info",
            };
            macro_start(name, out);
            out.push_str("<ac:rich-text-body>");
            blocks(&node.content, out);
            out.push_str("</ac:rich-text-body></ac:structured-macro>");
        }
        "expand" | "nestedExpand" => {
            macro_start("expand", out);
            let title = attr_str(node, "title");
            if !title.is_empty() {
                parameter("title", title, out);
            }
            out.push_str("<ac:rich-text-body>");
            blocks(&node.content, out);
            out.push_str("</ac:rich-text-body></ac:structured-macro>");
        }
        "table" => {
            out.push_str("<table><tbody>");
            blocks(&node.content, out);
            out.push_str("</tbody></table>");
        }
        "tableRow" => {
            out.push_str("<tr>");
            blocks(&node.content, out);
            out.push_str("</tr>");
        }
        "tableHeader" | "tableCell" => {
            let tag = if node.kind == "tableHeader" {
                "th"
            } else {
                "td"
            };
            out.push_str(&format!("<{tag}>"));
            blocks(&node.content, out);
            out.push_str(&format!("</{tag}>"));
        }
        "taskList" => {
            out.push_str("<ac:task-list>");
            blocks(&node.content, out);
            out.push_str("</ac:task-list>");
        }
        "taskItem" => {
            let status = if attr_str(node, "state") == "DONE" {
                "complete"
            } else {
                "incomplete"
            };
            out.push_str(&format!(
                "<ac:task><ac:task-status>{status}</ac:task-status><ac:task-body>"
            ));
            inlines(&node.content, out);
            out.push_str("</ac:task-body></ac:task>");
        }
        // Inline content at block level, or an unknown block: keep what it holds.
        _ if is_inline(&node.kind) => {
            out.push_str("<p>");
            inlines(std::slice::from_ref(node), out);
            out.push_str("</p>");
        }
        _ => blocks(&node.content, out),
    }
}

fn is_inline(kind: &str) -> bool {
    matches!(
        kind,
        "text" | "hardBreak" | "mention" | "emoji" | "status" | "inlineCard" | "date"
    )
}

/// Writes inline nodes. Neighbours that share their outer mark share one tag, so
/// `<em>a <code>b</code></em>` stays one `em` element.
fn inlines(nodes: &[Node], out: &mut String) {
    let items: Vec<(&[Mark], &Node)> = nodes.iter().map(|n| (n.marks.as_slice(), n)).collect();
    marked_run(&items, out);
}

fn marked_run(items: &[(&[Mark], &Node)], out: &mut String) {
    let mut i = 0;
    while i < items.len() {
        let (marks, node) = items[i];
        let Some(outer) = marks.first() else {
            inline(node, out);
            i += 1;
            continue;
        };
        // The run of neighbours with the same outer mark.
        let end = items[i..]
            .iter()
            .position(|(m, _)| m.first() != Some(outer))
            .map_or(items.len(), |p| i + p);
        let inner: Vec<(&[Mark], &Node)> =
            items[i..end].iter().map(|(m, n)| (&m[1..], *n)).collect();
        let mut text = String::new();
        marked_run(&inner, &mut text);
        out.push_str(&wrap_mark(outer, text));
        i = end;
    }
}

fn inline(node: &Node, out: &mut String) {
    match node.kind.as_str() {
        // Marks are applied by `marked_run`.
        "text" => out.push_str(&escape(node.text.as_deref().unwrap_or_default())),
        "hardBreak" => out.push_str("<br />"),
        "mention" => {
            let id = attr_str(node, "id");
            out.push_str(&format!(
                "<ac:link><ri:user ri:account-id=\"{}\" /></ac:link>",
                escape(id)
            ));
        }
        "emoji" => {
            let short = attr_str(node, "shortName");
            let fallback = attr_str(node, "text");
            out.push_str(&format!(
                "<ac:emoticon ac:name=\"blue-star\" ac:emoji-shortname=\"{}\" ac:emoji-fallback=\"{}\" />",
                escape(short),
                escape(fallback)
            ));
        }
        "status" => {
            macro_start("status", out);
            parameter("title", attr_str(node, "text"), out);
            let colour = match attr_str(node, "color") {
                "green" => "Green",
                "red" => "Red",
                "yellow" => "Yellow",
                "blue" => "Blue",
                "purple" => "Purple",
                _ => "Grey",
            };
            parameter("colour", colour, out);
            out.push_str("</ac:structured-macro>");
        }
        "inlineCard" => {
            let url = escape(attr_str(node, "url"));
            out.push_str(&format!("<a href=\"{url}\">{url}</a>"));
        }
        "date" => {
            // ADF dates are milliseconds since the epoch, as a string.
            let ms = attr_str(node, "timestamp");
            out.push_str(&format!("<time datetime=\"{}\" />", escape(ms)));
        }
        _ => inlines(&node.content, out),
    }
}

fn mark_attr<'a>(mark: &'a Mark, name: &str) -> &'a str {
    mark.attrs
        .as_ref()
        .and_then(|a| a.get(name))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

fn wrap_mark(mark: &Mark, inner: String) -> String {
    match mark.kind.as_str() {
        "strong" => format!("<strong>{inner}</strong>"),
        "em" => format!("<em>{inner}</em>"),
        "code" => format!("<code>{inner}</code>"),
        "strike" => format!("<s>{inner}</s>"),
        "underline" => format!("<u>{inner}</u>"),
        "subsup" if mark_attr(mark, "type") == "sup" => format!("<sup>{inner}</sup>"),
        "subsup" => format!("<sub>{inner}</sub>"),
        "link" => format!(
            "<a href=\"{}\">{inner}</a>",
            escape(mark_attr(mark, "href"))
        ),
        "textColor" => format!(
            "<span style=\"color: {};\">{inner}</span>",
            escape(mark_attr(mark, "color"))
        ),
        _ => inner,
    }
}

// Storage to ADF.

/// Reads a storage-format body into an ADF document.
pub fn from_storage(storage: &str) -> Node {
    let fragment = Html::parse_fragment(storage);
    let mut content = Vec::new();
    // `parse_fragment` wraps the input in an `<html>` element.
    let root = fragment.root_element();
    block_children(root, &mut content);
    Node::doc(content)
}

fn attrs(pairs: &[(&str, Value)]) -> Option<Map<String, Value>> {
    Some(
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
    )
}

fn node(kind: &str, content: Vec<Node>) -> Node {
    Node {
        content,
        ..Node::new(kind)
    }
}

/// The value of `<ac:parameter ac:name="name">` directly inside a macro.
fn macro_parameter(element: ElementRef, name: &str) -> Option<String> {
    element
        .children()
        .filter_map(ElementRef::wrap)
        .find(|e| e.value().name() == "ac:parameter" && e.value().attr("ac:name") == Some(name))
        .map(|e| e.text().collect())
}

/// The first child element with this tag name.
fn child<'a>(element: ElementRef<'a>, name: &str) -> Option<ElementRef<'a>> {
    element
        .children()
        .filter_map(ElementRef::wrap)
        .find(|e| e.value().name() == name)
}

/// Converts the children of `element` to block nodes. Runs of inline content become paragraphs.
fn block_children(element: ElementRef, out: &mut Vec<Node>) {
    let mut pending: Vec<Node> = Vec::new();
    let flush = |pending: &mut Vec<Node>, out: &mut Vec<Node>| {
        let blank = pending
            .iter()
            .all(|n| n.kind == "text" && n.text.as_deref().is_none_or(|t| t.trim().is_empty()));
        if !blank {
            out.push(Node::paragraph(trim_inline(std::mem::take(pending))));
        }
        pending.clear();
    };
    for child_node in element.children() {
        match child_node.value() {
            HtmlNode::Element(_) => {
                let child_element = ElementRef::wrap(child_node).expect("an element");
                if let Some(converted) = block_element(child_element) {
                    flush(&mut pending, out);
                    out.extend(converted);
                } else {
                    inline_element(child_element, &[], &mut pending);
                }
            }
            HtmlNode::Text(text) => push_text(&text.text, &[], &mut pending),
            HtmlNode::Comment(comment) => {
                // html5ever reads CDATA in HTML as a comment.
                if let Some(cdata) = cdata(&comment.comment) {
                    push_text(&cdata, &[], &mut pending);
                }
            }
            _ => {}
        }
    }
    flush(&mut pending, out);
}

fn cdata(comment: &str) -> Option<String> {
    comment
        .strip_prefix("[CDATA[")
        .map(|rest| rest.strip_suffix("]]").unwrap_or(rest).to_string())
}

/// The block nodes for a block-level element, or `None` when the element is inline.
fn block_element(element: ElementRef) -> Option<Vec<Node>> {
    let name = element.value().name();
    let inline_content = |element: ElementRef| {
        let mut content = Vec::new();
        inline_children(element, &[], &mut content);
        trim_inline(content)
    };
    let blocks_of = |element: ElementRef| {
        let mut content = Vec::new();
        block_children(element, &mut content);
        content
    };
    Some(match name {
        "p" => vec![Node::paragraph(inline_content(element))],
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            let level: u64 = name[1..].parse().unwrap_or(1);
            vec![Node {
                attrs: attrs(&[("level", json!(level))]),
                ..node("heading", inline_content(element))
            }]
        }
        "ul" | "ol" => {
            let items = element
                .children()
                .filter_map(ElementRef::wrap)
                .filter(|e| e.value().name() == "li")
                .map(|li| node("listItem", non_empty(blocks_of(li))))
                .collect();
            let mut list = node(
                if name == "ul" {
                    "bulletList"
                } else {
                    "orderedList"
                },
                items,
            );
            if name == "ol" {
                let start = element
                    .value()
                    .attr("start")
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(1);
                list.attrs = attrs(&[("order", json!(start))]);
            }
            vec![list]
        }
        "blockquote" => vec![node("blockquote", non_empty(blocks_of(element)))],
        "pre" => vec![code_block(element.text().collect(), None)],
        "hr" => vec![Node::new("rule")],
        "table" => vec![table(element)],
        "div" | "section" | "article" | "body" | "tbody" | "thead" | "ac:rich-text-body"
        | "ac:layout" | "ac:layout-section" | "ac:layout-cell" => blocks_of(element),
        "ac:structured-macro" | "ac:macro" => macro_block(element)?,
        "ac:task-list" => {
            let items = element
                .children()
                .filter_map(ElementRef::wrap)
                .filter(|e| e.value().name() == "ac:task")
                .map(|task| {
                    let done = child(task, "ac:task-status")
                        .is_some_and(|s| s.text().collect::<String>().trim() == "complete");
                    let content = child(task, "ac:task-body")
                        .map(inline_content)
                        .unwrap_or_default();
                    Node {
                        attrs: attrs(&[
                            ("localId", json!("")),
                            ("state", json!(if done { "DONE" } else { "TODO" })),
                        ]),
                        ..node("taskItem", content)
                    }
                })
                .collect();
            vec![Node {
                attrs: attrs(&[("localId", json!(""))]),
                ..node("taskList", items)
            }]
        }
        _ => return None,
    })
}

/// ADF requires list items, quotes, and cells to hold at least one block.
fn non_empty(content: Vec<Node>) -> Vec<Node> {
    if content.is_empty() {
        vec![Node::paragraph(Vec::new())]
    } else {
        content
    }
}

fn code_block(code: String, language: Option<String>) -> Node {
    let content = if code.is_empty() {
        Vec::new()
    } else {
        vec![Node::text(code)]
    };
    let mut block = node("codeBlock", content);
    if let Some(language) = language.filter(|l| !l.is_empty()) {
        block.attrs = attrs(&[("language", json!(language))]);
    }
    block
}

/// Block macros: code, panels, and expands. Inline macros (such as status) return `None`.
fn macro_block(element: ElementRef) -> Option<Vec<Node>> {
    let name = element.value().attr("ac:name").unwrap_or_default();
    let body = || {
        child(element, "ac:rich-text-body")
            .map(|b| {
                let mut content = Vec::new();
                block_children(b, &mut content);
                content
            })
            .unwrap_or_default()
    };
    Some(match name {
        "code" | "noformat" => {
            let code = child(element, "ac:plain-text-body")
                .map(|b| {
                    // The body is CDATA, which html5ever keeps as comments.
                    b.children()
                        .map(|n| match n.value() {
                            HtmlNode::Comment(c) => cdata(&c.comment).unwrap_or_default(),
                            HtmlNode::Text(t) => t.text.to_string(),
                            _ => String::new(),
                        })
                        .collect::<String>()
                })
                .unwrap_or_default();
            vec![code_block(code, macro_parameter(element, "language"))]
        }
        "info" | "note" | "warning" | "tip" => {
            let panel_type = match name {
                "note" => "note",
                "warning" => "error",
                "tip" => "success",
                _ => "info",
            };
            vec![Node {
                attrs: attrs(&[("panelType", json!(panel_type))]),
                ..node("panel", non_empty(body()))
            }]
        }
        "expand" => vec![Node {
            attrs: attrs(&[(
                "title",
                json!(macro_parameter(element, "title").unwrap_or_default()),
            )]),
            ..node("expand", non_empty(body()))
        }],
        "status" => return None,
        // Other macros have no ADF form yet; keep their text.
        _ => {
            let mut content = body();
            if content.is_empty() {
                let text: String = element.text().collect();
                if !text.trim().is_empty() {
                    content.push(Node::paragraph(vec![Node::text(text.trim())]));
                }
            }
            content
        }
    })
}

fn table(element: ElementRef) -> Node {
    let rows = element
        .descendants()
        .filter_map(ElementRef::wrap)
        .filter(|e| e.value().name() == "tr")
        .map(|tr| {
            let cells = tr
                .children()
                .filter_map(ElementRef::wrap)
                .filter(|c| matches!(c.value().name(), "td" | "th"))
                .map(|cell| {
                    let mut content = Vec::new();
                    block_children(cell, &mut content);
                    let kind = if cell.value().name() == "th" {
                        "tableHeader"
                    } else {
                        "tableCell"
                    };
                    node(kind, non_empty(content))
                })
                .collect();
            node("tableRow", cells)
        })
        .collect();
    node("table", rows)
}

/// Text with the given marks, merged with the text before it when the marks match.
fn push_text(text: &str, marks: &[Mark], out: &mut Vec<Node>) {
    if text.is_empty() {
        return;
    }
    // HTML collapses runs of whitespace; so does ADF rendering.
    let collapsed = collapse_whitespace(text);
    if let Some(last) = out.last_mut()
        && last.kind == "text"
        && last.marks == marks
    {
        last.text
            .get_or_insert_with(String::new)
            .push_str(&collapsed);
        return;
    }
    let mut node = Node::text(collapsed);
    node.marks = marks.to_vec();
    out.push(node);
}

fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            if !last_space {
                out.push(' ');
            }
            last_space = true;
        } else {
            out.push(c);
            last_space = false;
        }
    }
    out
}

/// Removes leading and trailing spaces of a paragraph's content.
fn trim_inline(mut content: Vec<Node>) -> Vec<Node> {
    if let Some(first) = content.first_mut()
        && first.kind == "text"
    {
        let text = first.text.take().unwrap_or_default();
        first.text = Some(text.trim_start().to_string());
    }
    if let Some(last) = content.last_mut()
        && last.kind == "text"
    {
        let text = last.text.take().unwrap_or_default();
        last.text = Some(text.trim_end().to_string());
    }
    content.retain(|n| n.kind != "text" || n.text.as_deref().is_some_and(|t| !t.is_empty()));
    content
}

fn inline_children(element: ElementRef, marks: &[Mark], out: &mut Vec<Node>) {
    for child_node in element.children() {
        match child_node.value() {
            HtmlNode::Text(text) => push_text(&text.text, marks, out),
            HtmlNode::Element(_) => inline_element(
                ElementRef::wrap(child_node).expect("an element"),
                marks,
                out,
            ),
            _ => {}
        }
    }
}

fn simple_mark(kind: &str) -> Mark {
    Mark {
        kind: kind.into(),
        attrs: None,
    }
}

fn inline_element(element: ElementRef, marks: &[Mark], out: &mut Vec<Node>) {
    let name = element.value().name();
    let with = |mark: Mark| {
        let mut next = marks.to_vec();
        next.push(mark);
        next
    };
    match name {
        "strong" | "b" => inline_children(element, &with(simple_mark("strong")), out),
        "em" | "i" => inline_children(element, &with(simple_mark("em")), out),
        "code" => inline_children(element, &with(simple_mark("code")), out),
        "s" | "del" | "strike" => inline_children(element, &with(simple_mark("strike")), out),
        "u" => inline_children(element, &with(simple_mark("underline")), out),
        "sub" | "sup" => {
            let mark = Mark {
                kind: "subsup".into(),
                attrs: attrs(&[("type", json!(name))]),
            };
            inline_children(element, &with(mark), out)
        }
        "a" => {
            let href = element.value().attr("href").unwrap_or_default();
            let mark = Mark {
                kind: "link".into(),
                attrs: attrs(&[("href", json!(href))]),
            };
            inline_children(element, &with(mark), out)
        }
        "span" => {
            let color = element
                .value()
                .attr("style")
                .and_then(|s| s.split(';').find_map(|d| d.trim().strip_prefix("color:")))
                .map(|c| c.trim().to_string());
            match color {
                Some(color) => {
                    let mark = Mark {
                        kind: "textColor".into(),
                        attrs: attrs(&[("color", json!(color))]),
                    };
                    inline_children(element, &with(mark), out)
                }
                None => inline_children(element, marks, out),
            }
        }
        "br" => out.push(Node::hard_break()),
        "ac:link" => {
            match child(element, "ri:user").and_then(|u| u.value().attr("ri:account-id")) {
                Some(id) => out.push(Node {
                    attrs: attrs(&[("id", json!(id)), ("text", json!(""))]),
                    ..Node::new("mention")
                }),
                None => inline_children(element, marks, out),
            }
        }
        "ac:emoticon" => {
            let short = element
                .value()
                .attr("ac:emoji-shortname")
                .map(str::to_string)
                .unwrap_or_else(|| {
                    format!(":{}:", element.value().attr("ac:name").unwrap_or("smile"))
                });
            let fallback = element
                .value()
                .attr("ac:emoji-fallback")
                .unwrap_or_default();
            out.push(Node {
                attrs: attrs(&[("shortName", json!(short)), ("text", json!(fallback))]),
                ..Node::new("emoji")
            });
        }
        "ac:structured-macro" if element.value().attr("ac:name") == Some("status") => {
            let colour = macro_parameter(element, "colour")
                .unwrap_or_default()
                .to_lowercase();
            let color = if colour.is_empty() {
                "neutral".to_string()
            } else {
                colour
            };
            out.push(Node {
                attrs: attrs(&[
                    (
                        "text",
                        json!(macro_parameter(element, "title").unwrap_or_default()),
                    ),
                    ("color", json!(color)),
                    ("localId", json!("")),
                ]),
                ..Node::new("status")
            });
        }
        // Unknown inline elements keep their text.
        _ => inline_children(element, marks, out),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(storage: &str) {
        let adf = from_storage(storage);
        assert_eq!(
            to_storage(&adf),
            storage,
            "via {}",
            serde_json::to_string(&adf).unwrap()
        );
    }

    #[test]
    fn round_trips_editor_output() {
        round_trip("<p>Hello <strong>bold</strong> and <em>italic <code>code</code></em>.</p>");
        round_trip(
            "<h2>Title</h2><ul><li><p>One</p></li><li><p>Two</p><ol><li><p>Inner</p></li></ol></li></ul>",
        );
        round_trip("<blockquote><p>Quoted</p></blockquote><hr />");
        round_trip("<p>A <a href=\"https://example.com\">link</a> and a<br />break</p>");
        round_trip(
            "<table><tbody><tr><th><p>Head</p></th></tr><tr><td><p>Cell</p></td></tr></tbody></table>",
        );
    }

    #[test]
    fn round_trips_macros() {
        round_trip(
            "<ac:structured-macro ac:name=\"code\" ac:schema-version=\"1\"><ac:parameter ac:name=\"language\">rust</ac:parameter><ac:plain-text-body><![CDATA[fn main() {\n    let a = 1 < 2;\n}]]></ac:plain-text-body></ac:structured-macro>",
        );
        round_trip(
            "<ac:structured-macro ac:name=\"info\" ac:schema-version=\"1\"><ac:rich-text-body><p>Note this</p></ac:rich-text-body></ac:structured-macro>",
        );
        round_trip(
            "<ac:task-list><ac:task><ac:task-status>complete</ac:task-status><ac:task-body>Done thing</ac:task-body></ac:task></ac:task-list>",
        );
        round_trip("<p><ac:link><ri:user ri:account-id=\"abc123\" /></ac:link> hi</p>");
    }

    #[test]
    fn reads_loose_storage() {
        let adf = from_storage("Just text\n<p>  spaced   out  </p>");
        assert_eq!(to_storage(&adf), "<p>Just text</p><p>spaced out</p>");
    }

    #[test]
    fn escapes_text() {
        let doc = Node::doc(vec![Node::paragraph(vec![Node::text("a < b & \"c\"")])]);
        assert_eq!(to_storage(&doc), "<p>a &lt; b &amp; &quot;c&quot;</p>");
        assert_eq!(from_storage(&to_storage(&doc)), doc);
    }
}
