//! ADF in a rich-text editor: drawing a document as editable HTML, and reading the edited
//! HTML back.
//!
//! The editor is a browser `contenteditable` element. [`to_editor_html`] writes what it shows;
//! [`from_editor`] reads it back through [`DomNode`], which the web app implements for the live
//! page. Nodes the editor cannot change (macros, images, expands, and the like) are drawn as
//! locked blocks that carry their ADF, so editing never loses them.

use serde_json::{Map, Value, json};

use crate::{Mark, Node};

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn attr<'a>(node: &'a Node, name: &str) -> Option<&'a Value> {
    node.attrs.as_ref()?.get(name)
}

fn attr_str<'a>(node: &'a Node, name: &str) -> &'a str {
    attr(node, name).and_then(Value::as_str).unwrap_or_default()
}

fn mark_str<'a>(mark: &'a Mark, name: &str) -> &'a str {
    mark.attrs
        .as_ref()
        .and_then(|a| a.get(name))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// A node kept as it is, in a locked element the editor shows but does not change.
fn locked(node: &Node, inline: bool, label: &str) -> String {
    let json = escape(&serde_json::to_string(node).unwrap_or_default());
    let tag = if inline { "span" } else { "div" };
    format!(
        "<{tag} class=\"editor-locked\" contenteditable=\"false\" data-adf=\"{json}\">{}</{tag}>",
        escape(label)
    )
}

/// Draws a document as editable HTML.
pub fn to_editor_html(doc: &Node) -> String {
    let mut out = String::new();
    for block in &doc.content {
        write_block(block, &mut out);
    }
    if out.is_empty() {
        out.push_str("<p><br></p>");
    }
    out
}

fn write_blocks(nodes: &[Node], out: &mut String) {
    for node in nodes {
        write_block(node, out);
    }
}

fn write_block(node: &Node, out: &mut String) {
    match node.kind.as_str() {
        "paragraph" => {
            out.push_str("<p>");
            if node.content.is_empty() {
                out.push_str("<br>");
            } else {
                write_inline(&node.content, out);
            }
            out.push_str("</p>");
        }
        "heading" => {
            let level = attr(node, "level")
                .and_then(Value::as_u64)
                .unwrap_or(1)
                .clamp(1, 6);
            out.push_str(&format!("<h{level}>"));
            write_inline(&node.content, out);
            out.push_str(&format!("</h{level}>"));
        }
        "bulletList" => {
            out.push_str("<ul>");
            write_blocks(&node.content, out);
            out.push_str("</ul>");
        }
        "orderedList" => {
            match attr(node, "order").and_then(Value::as_u64) {
                Some(start) if start != 1 => out.push_str(&format!("<ol start=\"{start}\">")),
                _ => out.push_str("<ol>"),
            }
            write_blocks(&node.content, out);
            out.push_str("</ol>");
        }
        "listItem" => {
            out.push_str("<li>");
            write_blocks(&node.content, out);
            out.push_str("</li>");
        }
        "taskList" => {
            out.push_str("<ul data-task-list=\"true\">");
            for item in &node.content {
                if item.kind == "taskItem" {
                    let done = attr_str(item, "state") == "DONE";
                    out.push_str(&format!(
                        "<li data-task=\"{}\"><input type=\"checkbox\" contenteditable=\"false\"{}>",
                        if done { "DONE" } else { "TODO" },
                        if done { " checked" } else { "" }
                    ));
                    write_inline(&item.content, out);
                    out.push_str("</li>");
                } else {
                    out.push_str(&locked(item, false, "Nested tasks"));
                }
            }
            out.push_str("</ul>");
        }
        "blockquote" => {
            out.push_str("<blockquote>");
            write_blocks(&node.content, out);
            out.push_str("</blockquote>");
        }
        "codeBlock" => {
            let text: String = node.content.iter().filter_map(|n| n.text.clone()).collect();
            out.push_str(&format!(
                "<pre data-language=\"{}\"><code>{}</code></pre>",
                escape(attr_str(node, "language")),
                escape(&text)
            ));
        }
        "rule" => out.push_str("<hr>"),
        "panel" => {
            let kind = match attr_str(node, "panelType") {
                "" => "info",
                other => other,
            };
            out.push_str(&format!(
                "<div class=\"editor-panel panel-{0}\" data-panel-type=\"{0}\">",
                escape(kind)
            ));
            write_blocks(&node.content, out);
            out.push_str("</div>");
        }
        "table" => {
            out.push_str("<table><tbody>");
            for row in &node.content {
                out.push_str("<tr>");
                for cell in &row.content {
                    let tag = if cell.kind == "tableHeader" {
                        "th"
                    } else {
                        "td"
                    };
                    let mut span = String::new();
                    for name in ["colspan", "rowspan"] {
                        if let Some(n) = attr(cell, name).and_then(Value::as_u64).filter(|n| *n > 1)
                        {
                            span.push_str(&format!(" {name}=\"{n}\""));
                        }
                    }
                    out.push_str(&format!("<{tag}{span}>"));
                    write_blocks(&cell.content, out);
                    out.push_str(&format!("</{tag}>"));
                }
                out.push_str("</tr>");
            }
            out.push_str("</tbody></table>");
        }
        "mediaSingle" | "mediaGroup" => out.push_str(&locked(node, false, "Image")),
        "expand" | "nestedExpand" => {
            let title = attr_str(node, "title");
            out.push_str(&locked(node, false, &format!("Expand: {title}")))
        }
        other => out.push_str(&locked(node, false, other)),
    }
}

fn wrap_mark(mark: &Mark, inner: String) -> String {
    match mark.kind.as_str() {
        "strong" => format!("<strong>{inner}</strong>"),
        "em" => format!("<em>{inner}</em>"),
        "underline" => format!("<u>{inner}</u>"),
        "strike" => format!("<s>{inner}</s>"),
        "code" => format!("<code>{inner}</code>"),
        "subsup" if mark_str(mark, "type") == "sup" => format!("<sup>{inner}</sup>"),
        "subsup" => format!("<sub>{inner}</sub>"),
        "link" => format!("<a href=\"{}\">{inner}</a>", escape(mark_str(mark, "href"))),
        "textColor" => format!(
            "<span style=\"color: {}\">{inner}</span>",
            escape(mark_str(mark, "color"))
        ),
        "annotation" => format!(
            "<span class=\"annotation\" data-annotation=\"{}\">{inner}</span>",
            escape(mark_str(mark, "id"))
        ),
        _ => inner,
    }
}

fn write_inline(nodes: &[Node], out: &mut String) {
    for node in nodes {
        let mut html = match node.kind.as_str() {
            "text" => escape(node.text.as_deref().unwrap_or_default()),
            "hardBreak" => "<br>".to_string(),
            "mention" => {
                let text = attr_str(node, "text");
                let shown = if text.is_empty() { "@someone" } else { text };
                format!(
                    "<span class=\"mention\" contenteditable=\"false\" data-mention-id=\"{}\">{}</span>",
                    escape(attr_str(node, "id")),
                    escape(shown)
                )
            }
            "emoji" => {
                let text = attr_str(node, "text");
                let label = if text.is_empty() {
                    attr_str(node, "shortName")
                } else {
                    text
                };
                locked(node, true, label)
            }
            "status" => locked(node, true, attr_str(node, "text")),
            "date" => locked(node, true, "Date"),
            "inlineCard" => locked(node, true, attr_str(node, "url")),
            "mediaInline" => locked(node, true, "Image"),
            other => locked(node, true, other),
        };
        // The innermost mark is last; wrap from the inside out.
        for mark in node.marks.iter().rev() {
            html = wrap_mark(mark, html);
        }
        out.push_str(&html);
    }
}

/// What a node of the edited page is.
pub enum DomKind {
    Text(String),
    Element,
    Other,
}

/// A node of the edited page. The web app implements it for the browser's DOM.
pub trait DomNode: Sized {
    fn kind(&self) -> DomKind;
    /// The lower-case tag name of an element.
    fn tag(&self) -> String;
    fn attr(&self, name: &str) -> Option<String>;
    fn children(&self) -> Vec<Self>;
    /// Whether a checkbox is ticked.
    fn checked(&self) -> bool;
}

const BLOCK_TAGS: &[&str] = &[
    "p",
    "div",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "ul",
    "ol",
    "li",
    "blockquote",
    "pre",
    "hr",
    "table",
    "tbody",
    "thead",
    "tr",
    "td",
    "th",
];

fn is_block<N: DomNode>(node: &N) -> bool {
    matches!(node.kind(), DomKind::Element) && BLOCK_TAGS.contains(&node.tag().as_str())
}

fn mark(kind: &str, attrs: Option<Map<String, Value>>) -> Mark {
    Mark {
        kind: kind.into(),
        attrs,
    }
}

fn attrs(pairs: &[(&str, Value)]) -> Option<Map<String, Value>> {
    Some(
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
    )
}

/// Reads the edited page back into a document.
pub fn from_editor<N: DomNode>(root: &N) -> Node {
    let mut content = read_blocks(&root.children());
    // Editors keep an empty line at the end for typing; it is not part of the document.
    while content.len() > 1
        && content
            .last()
            .is_some_and(|n| n.kind == "paragraph" && n.content.is_empty())
    {
        content.pop();
    }
    if content.is_empty() {
        content.push(Node::paragraph(Vec::new()));
    }
    Node::doc(content)
}

/// Block content: block elements, with any loose inline content gathered into paragraphs.
fn read_blocks<N: DomNode>(nodes: &[N]) -> Vec<Node> {
    let mut out = Vec::new();
    let mut loose: Vec<Node> = Vec::new();
    let flush = |loose: &mut Vec<Node>, out: &mut Vec<Node>| {
        let inline = tidy(std::mem::take(loose));
        if !inline.is_empty() {
            out.push(Node::paragraph(inline));
        }
    };
    for node in nodes {
        if is_block(node) {
            flush(&mut loose, &mut out);
            out.extend(read_block(node));
        } else {
            read_inline(node, &[], &mut loose);
        }
    }
    flush(&mut loose, &mut out);
    out
}

/// A stored node, from a locked element.
fn stored<N: DomNode>(node: &N) -> Option<Node> {
    serde_json::from_str(&node.attr("data-adf")?).ok()
}

fn read_block<N: DomNode>(node: &N) -> Vec<Node> {
    if let Some(kept) = stored(node) {
        return vec![kept];
    }
    let tag = node.tag();
    let children = node.children();
    let block = |kind: &str, content: Vec<Node>| Node {
        content,
        ..Node::new(kind)
    };
    match tag.as_str() {
        "p" => vec![Node::paragraph(read_inline_all(&children))],
        "div" => match node.attr("data-panel-type") {
            Some(kind) => vec![Node {
                attrs: attrs(&[("panelType", json!(kind))]),
                ..block("panel", non_empty(read_blocks(&children)))
            }],
            // Browsers wrap new lines in divs; their content stands on its own.
            None => read_blocks(&children),
        },
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            let level: u64 = tag[1..].parse().unwrap_or(1);
            vec![Node {
                attrs: attrs(&[("level", json!(level))]),
                ..block("heading", read_inline_all(&children))
            }]
        }
        "ul" if node.attr("data-task-list").is_some() => {
            let items = children
                .iter()
                .filter(|c| matches!(c.kind(), DomKind::Element) && c.tag() == "li")
                .map(|li| {
                    let done = li
                        .children()
                        .iter()
                        .find(|c| matches!(c.kind(), DomKind::Element) && c.tag() == "input")
                        .map(DomNode::checked)
                        .unwrap_or_else(|| li.attr("data-task").as_deref() == Some("DONE"));
                    Node {
                        attrs: attrs(&[
                            ("localId", json!("")),
                            ("state", json!(if done { "DONE" } else { "TODO" })),
                        ]),
                        ..block("taskItem", read_inline_all(&li.children()))
                    }
                })
                .collect();
            vec![Node {
                attrs: attrs(&[("localId", json!(""))]),
                ..block("taskList", items)
            }]
        }
        "ul" | "ol" => {
            let items: Vec<Node> = children
                .iter()
                .filter(|c| matches!(c.kind(), DomKind::Element) && c.tag() == "li")
                .map(|li| block("listItem", list_item_content(&li.children())))
                .collect();
            if items.is_empty() {
                return Vec::new();
            }
            let mut list = block(
                if tag == "ul" {
                    "bulletList"
                } else {
                    "orderedList"
                },
                items,
            );
            if tag == "ol"
                && let Some(start) = node.attr("start").and_then(|s| s.parse::<u64>().ok())
                && start != 1
            {
                list.attrs = attrs(&[("order", json!(start))]);
            }
            vec![list]
        }
        "li" => vec![block("listItem", list_item_content(&children))],
        "blockquote" => vec![block("blockquote", non_empty(read_blocks(&children)))],
        "pre" => {
            let text = plain_text(node);
            let mut code = block("codeBlock", Vec::new());
            if !text.is_empty() {
                code.content.push(Node::text(text.trim_end_matches('\n')));
            }
            if let Some(language) = node.attr("data-language").filter(|l| !l.is_empty()) {
                code.attrs = attrs(&[("language", json!(language))]);
            }
            vec![code]
        }
        "hr" => vec![Node::new("rule")],
        "table" | "tbody" | "thead" => {
            let rows = table_rows(node);
            if rows.is_empty() {
                Vec::new()
            } else {
                vec![block("table", rows)]
            }
        }
        _ => read_blocks(&children),
    }
}

fn table_rows<N: DomNode>(node: &N) -> Vec<Node> {
    let mut rows = Vec::new();
    for child in node.children() {
        if !matches!(child.kind(), DomKind::Element) {
            continue;
        }
        match child.tag().as_str() {
            "tbody" | "thead" => rows.extend(table_rows(&child)),
            "tr" => {
                let cells: Vec<Node> = child
                    .children()
                    .iter()
                    .filter(|c| {
                        matches!(c.kind(), DomKind::Element)
                            && matches!(c.tag().as_str(), "td" | "th")
                    })
                    .map(|cell| {
                        let mut pairs = Vec::new();
                        for name in ["colspan", "rowspan"] {
                            if let Some(n) = cell
                                .attr(name)
                                .and_then(|v| v.parse::<u64>().ok())
                                .filter(|n| *n > 1)
                            {
                                pairs.push((name, json!(n)));
                            }
                        }
                        Node {
                            attrs: if pairs.is_empty() {
                                None
                            } else {
                                attrs(&pairs)
                            },
                            content: non_empty(read_blocks(&cell.children())),
                            ..Node::new(if cell.tag() == "th" {
                                "tableHeader"
                            } else {
                                "tableCell"
                            })
                        }
                    })
                    .collect();
                if !cells.is_empty() {
                    rows.push(Node {
                        content: cells,
                        ..Node::new("tableRow")
                    });
                }
            }
            _ => {}
        }
    }
    rows
}

/// A list item's content, which must start with a paragraph.
fn list_item_content<N: DomNode>(children: &[N]) -> Vec<Node> {
    let mut content = read_blocks(children);
    if content.first().is_none_or(|n| n.kind != "paragraph") {
        content.insert(0, Node::paragraph(Vec::new()));
    }
    content
}

/// Block content that is never empty, as cells, panels, and quotes need.
fn non_empty(mut content: Vec<Node>) -> Vec<Node> {
    if content.is_empty() {
        content.push(Node::paragraph(Vec::new()));
    }
    content
}

fn plain_text<N: DomNode>(node: &N) -> String {
    let mut out = String::new();
    for child in node.children() {
        match child.kind() {
            DomKind::Text(text) => out.push_str(&text),
            DomKind::Element if child.tag() == "br" => out.push('\n'),
            DomKind::Element => out.push_str(&plain_text(&child)),
            DomKind::Other => {}
        }
    }
    out
}

fn read_inline_all<N: DomNode>(nodes: &[N]) -> Vec<Node> {
    let mut out = Vec::new();
    for node in nodes {
        read_inline(node, &[], &mut out);
    }
    tidy(out)
}

fn read_inline<N: DomNode>(node: &N, marks: &[Mark], out: &mut Vec<Node>) {
    let text = match node.kind() {
        DomKind::Text(text) => text,
        DomKind::Other => return,
        DomKind::Element => {
            return read_inline_element(node, marks, out);
        }
    };
    // Editors use non-breaking spaces to keep spaces visible, and zero-width spaces to give
    // the caret a place in new empty blocks.
    let text = text.replace('\u{a0}', " ").replace('\u{200b}', "");
    if !text.is_empty() {
        out.push(Node {
            marks: marks.to_vec(),
            ..Node::text(text)
        });
    }
}

fn read_inline_element<N: DomNode>(node: &N, marks: &[Mark], out: &mut Vec<Node>) {
    if let Some(mut kept) = stored(node) {
        kept.marks = marks.to_vec();
        out.push(kept);
        return;
    }
    if let Some(id) = node.attr("data-mention-id") {
        out.push(Node {
            attrs: attrs(&[("id", json!(id)), ("text", json!(plain_text(node)))]),
            ..Node::new("mention")
        });
        return;
    }
    let tag = node.tag();
    let with = |mark: Mark| {
        let mut next = marks.to_vec();
        if !next.contains(&mark) {
            next.push(mark);
        }
        next
    };
    let next = match tag.as_str() {
        "br" => {
            out.push(Node::hard_break());
            return;
        }
        // A task box is read by its list.
        "input" => return,
        "strong" | "b" => with(mark("strong", None)),
        "em" | "i" => with(mark("em", None)),
        "u" => with(mark("underline", None)),
        "s" | "strike" | "del" => with(mark("strike", None)),
        "code" => with(mark("code", None)),
        "sub" | "sup" => with(mark("subsup", attrs(&[("type", json!(tag))]))),
        "a" => match node.attr("href") {
            Some(href) => with(mark("link", attrs(&[("href", json!(href))]))),
            None => marks.to_vec(),
        },
        "span" | "font" => {
            let mut next = marks.to_vec();
            if let Some(id) = node.attr("data-annotation") {
                next = with(mark(
                    "annotation",
                    attrs(&[
                        ("id", json!(id)),
                        ("annotationType", json!("inlineComment")),
                    ]),
                ));
            }
            let color = node
                .attr("style")
                .and_then(|s| {
                    s.split(';').find_map(|d| {
                        d.trim()
                            .strip_prefix("color:")
                            .map(|c| c.trim().to_string())
                    })
                })
                .or_else(|| node.attr("color"));
            if let Some(color) = color.filter(|c| !c.is_empty()) {
                next.retain(|m| m.kind != "textColor");
                next.push(mark("textColor", attrs(&[("color", json!(color))])));
            }
            next
        }
        _ => marks.to_vec(),
    };
    for child in node.children() {
        read_inline(&child, &next, out);
    }
}

/// Joins neighbouring text with the same marks, and drops a trailing line break, which
/// editors add to keep empty lines open.
fn tidy(nodes: Vec<Node>) -> Vec<Node> {
    let mut out: Vec<Node> = Vec::new();
    for node in nodes {
        if let Some(last) = out.last_mut()
            && last.kind == "text"
            && node.kind == "text"
            && last.marks == node.marks
        {
            let joined = format!(
                "{}{}",
                last.text.as_deref().unwrap_or_default(),
                node.text.as_deref().unwrap_or_default()
            );
            last.text = Some(joined);
            continue;
        }
        out.push(node);
    }
    if out.last().is_some_and(|n| n.kind == "hardBreak") {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny DOM for tests.
    #[derive(Clone)]
    enum Fake {
        Text(&'static str),
        El(&'static str, Vec<(&'static str, String)>, Vec<Fake>),
    }

    fn el(tag: &'static str, children: Vec<Fake>) -> Fake {
        Fake::El(tag, Vec::new(), children)
    }

    fn el_with(tag: &'static str, attrs: Vec<(&'static str, &str)>, children: Vec<Fake>) -> Fake {
        Fake::El(
            tag,
            attrs.into_iter().map(|(k, v)| (k, v.to_string())).collect(),
            children,
        )
    }

    impl DomNode for Fake {
        fn kind(&self) -> DomKind {
            match self {
                Fake::Text(t) => DomKind::Text(t.to_string()),
                Fake::El(..) => DomKind::Element,
            }
        }
        fn tag(&self) -> String {
            match self {
                Fake::El(tag, ..) => tag.to_string(),
                Fake::Text(_) => String::new(),
            }
        }
        fn attr(&self, name: &str) -> Option<String> {
            match self {
                Fake::El(_, attrs, _) => attrs
                    .iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| v.clone()),
                Fake::Text(_) => None,
            }
        }
        fn children(&self) -> Vec<Self> {
            match self {
                Fake::El(_, _, children) => children.clone(),
                Fake::Text(_) => Vec::new(),
            }
        }
        fn checked(&self) -> bool {
            self.attr("checked").is_some()
        }
    }

    #[test]
    fn reads_what_editors_write() {
        let root = el(
            "div",
            vec![
                el("h2", vec![Fake::Text("Plan")]),
                el(
                    "p",
                    vec![
                        Fake::Text("Ship "),
                        el("b", vec![Fake::Text("today")]),
                        el("br", vec![]),
                    ],
                ),
                // Browsers put new lines in divs, and loose text right in the root.
                el("div", vec![Fake::Text("Second\u{a0}line")]),
                Fake::Text("loose"),
                el(
                    "ul",
                    vec![
                        el("li", vec![Fake::Text("one")]),
                        el("li", vec![el("p", vec![Fake::Text("two")])]),
                    ],
                ),
                el_with(
                    "ul",
                    vec![("data-task-list", "true")],
                    vec![el_with(
                        "li",
                        vec![("data-task", "TODO")],
                        vec![
                            el_with("input", vec![("checked", "")], vec![]),
                            Fake::Text("Done thing"),
                        ],
                    )],
                ),
                el(
                    "table",
                    vec![el(
                        "tbody",
                        vec![el(
                            "tr",
                            vec![el("th", vec![Fake::Text("Head")]), el("td", vec![])],
                        )],
                    )],
                ),
            ],
        );
        let doc = from_editor(&root);
        let json = serde_json::to_value(&doc).unwrap();
        assert_eq!(json["content"][0]["type"], "heading");
        assert_eq!(json["content"][0]["attrs"]["level"], 2);
        let p = &json["content"][1]["content"];
        assert_eq!(p[1]["marks"][0]["type"], "strong");
        assert_eq!(
            p.as_array().unwrap().len(),
            2,
            "the trailing break is dropped"
        );
        assert_eq!(json["content"][2]["content"][0]["text"], "Second line");
        assert_eq!(json["content"][3]["content"][0]["text"], "loose");
        assert_eq!(json["content"][4]["type"], "bulletList");
        assert_eq!(
            json["content"][4]["content"][1]["content"][0]["type"],
            "paragraph"
        );
        assert_eq!(json["content"][5]["content"][0]["attrs"]["state"], "DONE");
        assert_eq!(
            json["content"][5]["content"][0]["content"][0]["text"],
            "Done thing"
        );
        assert_eq!(
            json["content"][6]["content"][0]["content"][1]["content"][0]["type"],
            "paragraph"
        );
    }

    #[test]
    fn locked_nodes_and_marks_survive() {
        let panel = serde_json::from_value::<Node>(json!({ "type": "expand", "attrs": { "title": "More" }, "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": "Hidden" }] }] })).unwrap();
        let doc = Node::doc(vec![
            panel.clone(),
            Node::paragraph(vec![Node {
                marks: vec![mark(
                    "annotation",
                    attrs(&[
                        ("id", json!("c1")),
                        ("annotationType", json!("inlineComment")),
                    ]),
                )],
                ..Node::text("noted")
            }]),
        ]);
        let html = to_editor_html(&doc);
        assert!(html.contains("data-adf="));
        assert!(html.contains("data-annotation=\"c1\""));
        // Reading the locked block back gives the same node.
        let stored_json = serde_json::to_string(&panel).unwrap();
        let root = el(
            "div",
            vec![
                el_with(
                    "div",
                    vec![("data-adf", &stored_json)],
                    vec![Fake::Text("Expand: More")],
                ),
                el(
                    "p",
                    vec![el_with(
                        "span",
                        vec![("data-annotation", "c1")],
                        vec![Fake::Text("noted")],
                    )],
                ),
            ],
        );
        let back = from_editor(&root);
        assert_eq!(back.content[0], panel);
        assert_eq!(back.content[1].content[0].marks[0].kind, "annotation");
    }
}
