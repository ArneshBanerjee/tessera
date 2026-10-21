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

use crate::{ImageSource, Mark, Node};

// ADF to storage, and to HTML for display.

/// Writes an ADF document as storage-format XHTML.
pub fn to_storage(doc: &Node) -> String {
    let writer = Writer {
        view: false,
        names: &|_| None,
        attachment_url: &|_| None,
    };
    let mut out = String::new();
    writer.blocks(&doc.content, &mut out);
    out
}

/// Writes an ADF document as plain HTML for display, with macros drawn as HTML: code blocks
/// as `pre`, panels as `div.panel`, expands as `details`, and so on. `names` gives the display
/// name for a mention's account id, and `attachment_url` the address of an attached image.
/// The output is not sanitized; callers must sanitize it.
pub fn to_view(
    doc: &Node,
    names: &dyn Fn(&str) -> Option<String>,
    attachment_url: &dyn Fn(&str) -> Option<String>,
) -> String {
    let writer = Writer {
        view: true,
        names,
        attachment_url,
    };
    let mut out = String::new();
    writer.blocks(&doc.content, &mut out);
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

fn is_inline(kind: &str) -> bool {
    matches!(
        kind,
        "text"
            | "hardBreak"
            | "mention"
            | "emoji"
            | "status"
            | "inlineCard"
            | "date"
            | "mediaInline"
    )
}

/// Writes ADF as storage (`view: false`) or as display HTML (`view: true`). The two differ
/// only in how macros are written.
struct Writer<'a> {
    view: bool,
    names: &'a dyn Fn(&str) -> Option<String>,
    attachment_url: &'a dyn Fn(&str) -> Option<String>,
}

impl Writer<'_> {
    fn blocks(&self, nodes: &[Node], out: &mut String) {
        for node in nodes {
            self.block(node, out);
        }
    }

    /// Wraps the node's blocks in a tag.
    fn wrapped(&self, open: &str, close: &str, node: &Node, out: &mut String) {
        out.push_str(open);
        self.blocks(&node.content, out);
        out.push_str(close);
    }

    fn block(&self, node: &Node, out: &mut String) {
        match node.kind.as_str() {
            "paragraph" => {
                out.push_str("<p>");
                self.inlines(&node.content, out);
                out.push_str("</p>");
            }
            "heading" => {
                let level = attr(node, "level")
                    .and_then(Value::as_u64)
                    .unwrap_or(1)
                    .clamp(1, 6);
                out.push_str(&format!("<h{level}>"));
                self.inlines(&node.content, out);
                out.push_str(&format!("</h{level}>"));
            }
            "bulletList" => self.wrapped("<ul>", "</ul>", node, out),
            "orderedList" => {
                let open = match attr(node, "order").and_then(Value::as_u64) {
                    Some(start) if start != 1 => format!("<ol start=\"{start}\">"),
                    _ => "<ol>".to_string(),
                };
                self.wrapped(&open, "</ol>", node, out);
            }
            "listItem" => self.wrapped("<li>", "</li>", node, out),
            "blockquote" => self.wrapped("<blockquote>", "</blockquote>", node, out),
            "codeBlock" => self.code_block(node, out),
            "rule" => out.push_str("<hr />"),
            "mediaSingle" if self.view => {
                out.push_str("<p class=\"media\">");
                self.image(node, out);
                out.push_str("</p>");
            }
            "mediaSingle" => self.image(node, out),
            "panel" => {
                let kind = attr_str(node, "panelType");
                if self.view {
                    let kind = if kind.is_empty() { "info" } else { kind };
                    let open = format!("<div class=\"panel panel-{}\">", escape(kind));
                    self.wrapped(&open, "</div>", node, out);
                } else {
                    let name = match kind {
                        "note" => "note",
                        "warning" | "error" => "warning",
                        "success" => "tip",
                        _ => "info",
                    };
                    macro_start(name, out);
                    self.wrapped(
                        "<ac:rich-text-body>",
                        "</ac:rich-text-body></ac:structured-macro>",
                        node,
                        out,
                    );
                }
            }
            "expand" | "nestedExpand" => {
                let title = attr_str(node, "title");
                if self.view {
                    let label = if title.is_empty() {
                        "Click here to expand…"
                    } else {
                        title
                    };
                    let open = format!(
                        "<details class=\"expand\"><summary>{}</summary>",
                        escape(label)
                    );
                    self.wrapped(&open, "</details>", node, out);
                } else {
                    macro_start("expand", out);
                    if !title.is_empty() {
                        parameter("title", title, out);
                    }
                    self.wrapped(
                        "<ac:rich-text-body>",
                        "</ac:rich-text-body></ac:structured-macro>",
                        node,
                        out,
                    );
                }
            }
            "table" => self.wrapped("<table><tbody>", "</tbody></table>", node, out),
            "tableRow" => self.wrapped("<tr>", "</tr>", node, out),
            "tableHeader" => self.wrapped("<th>", "</th>", node, out),
            "tableCell" => self.wrapped("<td>", "</td>", node, out),
            "taskList" if self.view => self.wrapped("<ul class=\"task-list\">", "</ul>", node, out),
            "taskList" => self.wrapped("<ac:task-list>", "</ac:task-list>", node, out),
            "taskItem" => {
                let done = attr_str(node, "state") == "DONE";
                if self.view {
                    let checked = if done { " checked=\"checked\"" } else { "" };
                    out.push_str(&format!(
                        "<li class=\"task\"><input type=\"checkbox\" disabled=\"disabled\"{checked} /> "
                    ));
                    self.inlines(&node.content, out);
                    out.push_str("</li>");
                } else {
                    let status = if done { "complete" } else { "incomplete" };
                    out.push_str(&format!(
                        "<ac:task><ac:task-status>{status}</ac:task-status><ac:task-body>"
                    ));
                    self.inlines(&node.content, out);
                    out.push_str("</ac:task-body></ac:task>");
                }
            }
            // Inline content at block level, or an unknown block: keep what it holds.
            _ if is_inline(&node.kind) => {
                out.push_str("<p>");
                self.inlines(std::slice::from_ref(node), out);
                out.push_str("</p>");
            }
            _ => self.blocks(&node.content, out),
        }
    }

    /// An image: `ac:image` in storage, `img` in the view.
    fn image(&self, node: &Node, out: &mut String) {
        let Some((source, alt)) = node.image_source() else {
            return;
        };
        let alt_attr = if alt.is_empty() {
            String::new()
        } else {
            format!(" ac:alt=\"{}\"", escape(alt))
        };
        if !self.view {
            let target = match source {
                ImageSource::Attachment(name) => {
                    format!("<ri:attachment ri:filename=\"{}\" />", escape(name))
                }
                ImageSource::Url(url) => format!("<ri:url ri:value=\"{}\" />", escape(url)),
            };
            out.push_str(&format!("<ac:image{alt_attr}>{target}</ac:image>"));
            return;
        }
        let src = match source {
            ImageSource::Attachment(name) => (self.attachment_url)(name),
            ImageSource::Url(url) => Some(url.to_string()),
        };
        match src {
            Some(src) => out.push_str(&format!(
                "<img src=\"{}\" alt=\"{}\" />",
                escape(&src),
                escape(alt)
            )),
            // Without an address, the alt text stands in.
            None => out.push_str(&escape(if alt.is_empty() { source.text() } else { alt })),
        }
    }

    fn code_block(&self, node: &Node, out: &mut String) {
        let code: String = node
            .content
            .iter()
            .filter_map(|n| n.text.as_deref())
            .collect();
        let language = attr_str(node, "language");
        if self.view {
            let class = if language.is_empty() {
                String::new()
            } else {
                format!(" class=\"language-{}\"", escape(language))
            };
            out.push_str(&format!("<pre><code{class}>{}</code></pre>", escape(&code)));
            return;
        }
        macro_start("code", out);
        if !language.is_empty() {
            parameter("language", language, out);
        }
        // CDATA cannot contain its own end marker, so split it where it appears.
        let safe = code.replace("]]>", "]]]]><![CDATA[>");
        out.push_str(&format!(
            "<ac:plain-text-body><![CDATA[{safe}]]></ac:plain-text-body></ac:structured-macro>"
        ));
    }

    /// Writes inline nodes. Neighbours that share their outer mark share one tag, so
    /// `<em>a <code>b</code></em>` stays one `em` element.
    fn inlines(&self, nodes: &[Node], out: &mut String) {
        let items: Vec<(&[Mark], &Node)> = nodes.iter().map(|n| (n.marks.as_slice(), n)).collect();
        self.marked_run(&items, out);
    }

    fn marked_run(&self, items: &[(&[Mark], &Node)], out: &mut String) {
        let mut i = 0;
        while i < items.len() {
            let (marks, node) = items[i];
            let Some(outer) = marks.first() else {
                self.inline(node, out);
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
            self.marked_run(&inner, &mut text);
            out.push_str(&wrap_mark(outer, text, self.view));
            i = end;
        }
    }

    fn inline(&self, node: &Node, out: &mut String) {
        match node.kind.as_str() {
            // Marks are applied by `marked_run`.
            "text" => out.push_str(&escape(node.text.as_deref().unwrap_or_default())),
            "hardBreak" => out.push_str("<br />"),
            "mention" => {
                let id = attr_str(node, "id");
                if self.view {
                    let name = (self.names)(id).unwrap_or_else(|| {
                        attr_str(node, "text").trim_start_matches('@').to_string()
                    });
                    out.push_str(&format!(
                        "<span class=\"mention\">@{}</span>",
                        escape(&name)
                    ));
                } else {
                    out.push_str(&format!(
                        "<ac:link><ri:user ri:account-id=\"{}\" /></ac:link>",
                        escape(id)
                    ));
                }
            }
            "emoji" => {
                let short = attr_str(node, "shortName");
                let fallback = attr_str(node, "text");
                if self.view {
                    let shown = if fallback.is_empty() { short } else { fallback };
                    out.push_str(&escape(shown));
                } else {
                    out.push_str(&format!(
                        "<ac:emoticon ac:name=\"blue-star\" ac:emoji-shortname=\"{}\" ac:emoji-fallback=\"{}\" />",
                        escape(short),
                        escape(fallback)
                    ));
                }
            }
            "status" => {
                let text = attr_str(node, "text");
                let color = attr_str(node, "color");
                if self.view {
                    let color = if color.is_empty() { "neutral" } else { color };
                    out.push_str(&format!(
                        "<span class=\"status status-{}\">{}</span>",
                        escape(color),
                        escape(text)
                    ));
                } else {
                    macro_start("status", out);
                    parameter("title", text, out);
                    let colour = match color {
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
            }
            "mediaInline" | "media" => self.image(node, out),
            "inlineCard" => {
                let url = escape(attr_str(node, "url"));
                out.push_str(&format!("<a href=\"{url}\">{url}</a>"));
            }
            "date" => {
                // ADF dates are milliseconds since the epoch, as a string.
                let ms = attr_str(node, "timestamp");
                out.push_str(&format!("<time datetime=\"{}\" />", escape(ms)));
            }
            _ => self.inlines(&node.content, out),
        }
    }
}

fn mark_attr<'a>(mark: &'a Mark, name: &str) -> &'a str {
    mark.attrs
        .as_ref()
        .and_then(|a| a.get(name))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

fn wrap_mark(mark: &Mark, inner: String, view: bool) -> String {
    match mark.kind.as_str() {
        // Text an inline comment is on.
        "annotation" if view => format!(
            "<span class=\"inline-comment-marker\" data-ref=\"{}\">{inner}</span>",
            escape(mark_attr(mark, "id"))
        ),
        "annotation" => format!(
            "<ac:inline-comment-marker ac:ref=\"{}\">{inner}</ac:inline-comment-marker>",
            escape(mark_attr(mark, "id"))
        ),
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
    let fragment = Html::parse_fragment(&cdata_to_text(storage));
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
            let paragraph = Node::paragraph(trim_inline(std::mem::take(pending)));
            out.push(lone_image_to_block(paragraph));
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
            _ => {}
        }
    }
    flush(&mut pending, out);
}

/// Replaces `<![CDATA[...]]>` sections with escaped text. An HTML parser reads CDATA as a
/// comment that ends at the first `>`, which would cut code such as `a > b` short.
fn cdata_to_text(storage: &str) -> String {
    let mut out = String::with_capacity(storage.len());
    let mut rest = storage;
    while let Some(start) = rest.find("<![CDATA[") {
        out.push_str(&rest[..start]);
        let body = &rest[start + 9..];
        let end = body.find("]]>").unwrap_or(body.len());
        out.push_str(&escape(&body[..end]));
        rest = body.get(end + 3..).unwrap_or_default();
    }
    out.push_str(rest);
    out
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
        "p" => vec![lone_image_to_block(Node::paragraph(inline_content(
            element,
        )))],
        "ac:image" => vec![Node::image_block(image_node(element))],
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
        // Active and hidden content has no place in a document.
        "script" | "style" | "template" | "noscript" | "iframe" | "object" => Vec::new(),
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

/// An `ac:image` as an inline image node.
fn image_node(element: ElementRef) -> Node {
    let alt = element.value().attr("ac:alt").unwrap_or_default();
    let source = child(element, "ri:attachment")
        .and_then(|a| a.value().attr("ri:filename"))
        .or_else(|| child(element, "ri:url").and_then(|u| u.value().attr("ri:value")))
        .unwrap_or_default();
    Node::image(source, alt)
}

/// A paragraph that holds only an image becomes an image block, as Confluence's editor
/// writes it.
fn lone_image_to_block(mut paragraph: Node) -> Node {
    if paragraph.content.len() == 1 && paragraph.content[0].kind == "mediaInline" {
        Node::image_block(paragraph.content.remove(0))
    } else {
        paragraph
    }
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
                // CDATA became text before parsing.
                .map(|b| b.text().collect::<String>())
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
        "ac:inline-comment-marker" => {
            let reference = element.value().attr("ac:ref").unwrap_or_default();
            let mark = Mark {
                kind: "annotation".into(),
                attrs: attrs(&[
                    ("id", json!(reference)),
                    ("annotationType", json!("inlineComment")),
                ]),
            };
            inline_children(element, &with(mark), out)
        }
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
        "ac:image" => out.push(image_node(element)),
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
    fn keeps_inline_comment_markers() {
        round_trip(
            "<p>Ship <ac:inline-comment-marker ac:ref=\"c1\">the <strong>new</strong> build</ac:inline-comment-marker> today.</p>",
        );
        let adf = from_storage(
            "<p><ac:inline-comment-marker ac:ref=\"c1\">noted</ac:inline-comment-marker></p>",
        );
        let mark = &adf.content[0].content[0].marks[0];
        assert_eq!(mark.kind, "annotation");
        assert_eq!(mark.attrs.as_ref().unwrap()["id"], "c1");
        let view = to_view(&adf, &|_| None, &|_| None);
        assert!(
            view.contains("<span class=\"inline-comment-marker\" data-ref=\"c1\">noted</span>"),
            "{view}"
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
    fn view_draws_macros_as_html() {
        let storage = "<ac:structured-macro ac:name=\"code\" ac:schema-version=\"1\"><ac:parameter ac:name=\"language\">sh</ac:parameter><ac:plain-text-body><![CDATA[echo <hi>]]></ac:plain-text-body></ac:structured-macro><p><ac:link><ri:user ri:account-id=\"u1\" /></ac:link> see</p>";
        let names = |id: &str| (id == "u1").then(|| "Jane".to_string());
        assert_eq!(
            to_view(&from_storage(storage), &names, &|_| None),
            "<pre><code class=\"language-sh\">echo &lt;hi&gt;</code></pre><p><span class=\"mention\">@Jane</span> see</p>"
        );
    }

    #[test]
    fn images_in_storage_and_view() {
        round_trip(
            "<ac:image ac:alt=\"Diagram\"><ri:attachment ri:filename=\"arch.png\" /></ac:image>",
        );
        round_trip(
            "<p>Logo <ac:image><ri:url ri:value=\"https://e.com/l.png\" /></ac:image> here</p>",
        );
        let doc =
            from_storage("<p><ac:image><ri:attachment ri:filename=\"a b.png\" /></ac:image></p>");
        assert_eq!(doc.content[0].kind, "mediaSingle");
        let url = |name: &str| Some(format!("/files/{name}"));
        assert_eq!(
            to_view(&doc, &|_| None, &url),
            "<p class=\"media\"><img src=\"/files/a b.png\" alt=\"\" /></p>"
        );
    }

    #[test]
    fn escapes_text() {
        let doc = Node::doc(vec![Node::paragraph(vec![Node::text("a < b & \"c\"")])]);
        assert_eq!(to_storage(&doc), "<p>a &lt; b &amp; &quot;c&quot;</p>");
        assert_eq!(from_storage(&to_storage(&doc)), doc);
    }
}
