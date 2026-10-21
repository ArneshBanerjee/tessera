//! Jira wiki markup to ADF, for text from Jira backups and the v2 API's older clients.
//!
//! Supported blocks: `h1.` to `h6.`, paragraphs, `*` `#` `-` lists (nested), `||` and `|`
//! tables, `bq.`, `----`, `{code}`, `{noformat}`, `{quote}`, `{panel}`, and `{info}`,
//! `{note}`, `{warning}`, `{tip}`. Inline: `*bold*`, `_italic_`, `-strike-`, `+underline+`,
//! `^sup^`, `~sub~`, `??citation??`, `{{monospace}}`, `{color:red}text{color}`, `[text|url]`,
//! `[url]`, `[~accountid:id]` mentions, `!image.png!`, bare URLs, and `\\` line breaks.

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

fn block(kind: &str, content: Vec<Node>) -> Node {
    Node {
        content,
        ..Node::new(kind)
    }
}

/// Converts Jira wiki markup to an ADF document.
pub fn from_wiki_markup(markup: &str) -> Node {
    let text = markup.replace("\r\n", "\n");
    let lines: Vec<&str> = text.split('\n').collect();
    let mut content = parse_blocks(&lines);
    if content.is_empty() {
        content.push(Node::paragraph(Vec::new()));
    }
    Node::doc(content)
}

/// The tag and parameters of a `{macro:params}` line, if the line is one.
fn macro_open(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    let inner = trimmed.strip_prefix('{')?;
    let end = inner.find('}')?;
    let head = &inner[..end];
    let (name, params) = match head.split_once(':') {
        Some((n, p)) => (n, p),
        None => (head, ""),
    };
    let name = name.trim().to_lowercase();
    matches!(
        name.as_str(),
        "code" | "noformat" | "quote" | "panel" | "info" | "note" | "warning" | "tip"
    )
    .then(|| (name, params.to_string()))
}

fn parse_blocks(lines: &[&str]) -> Vec<Node> {
    let mut out = Vec::new();
    let mut paragraph: Vec<&str> = Vec::new();
    let flush = |paragraph: &mut Vec<&str>, out: &mut Vec<Node>| {
        if !paragraph.is_empty() {
            let joined = paragraph.join("\n");
            out.push(Node::paragraph(parse_inline(&joined)));
            paragraph.clear();
        }
    };
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        if trimmed.is_empty() {
            flush(&mut paragraph, &mut out);
            i += 1;
            continue;
        }
        // Macros that hold several lines.
        if let Some((name, params)) = macro_open(trimmed) {
            flush(&mut paragraph, &mut out);
            let close = format!("{{{name}}}");
            // The opening tag may carry text after it on the same line.
            let open_end = trimmed.find('}').map_or(trimmed.len(), |e| e + 1);
            let first = &trimmed[open_end..];
            let mut body: Vec<String> = Vec::new();
            let mut j = i;
            let mut closed = false;
            if let Some(end) = first.to_lowercase().find(&close) {
                body.push(first[..end].to_string());
                closed = true;
            } else {
                if !first.is_empty() {
                    body.push(first.to_string());
                }
                j += 1;
                while j < lines.len() {
                    if let Some(end) = lines[j].to_lowercase().find(&close) {
                        body.push(lines[j][..end].to_string());
                        closed = true;
                        break;
                    }
                    body.push(lines[j].to_string());
                    j += 1;
                }
            }
            if !closed {
                // An unclosed macro is just text.
                paragraph.push(line);
                i += 1;
                continue;
            }
            out.push(macro_block(&name, &params, &body));
            i = j + 1;
            continue;
        }
        if let Some(rest) = heading(trimmed) {
            flush(&mut paragraph, &mut out);
            out.push(rest);
            i += 1;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("bq.") {
            flush(&mut paragraph, &mut out);
            out.push(block(
                "blockquote",
                vec![Node::paragraph(parse_inline(rest.trim()))],
            ));
            i += 1;
            continue;
        }
        if trimmed.len() >= 4 && trimmed.chars().all(|c| c == '-') {
            flush(&mut paragraph, &mut out);
            out.push(Node::new("rule"));
            i += 1;
            continue;
        }
        if trimmed.starts_with('|') {
            flush(&mut paragraph, &mut out);
            let mut rows = Vec::new();
            while i < lines.len() && lines[i].trim().starts_with('|') {
                rows.push(table_row(lines[i].trim()));
                i += 1;
            }
            out.push(block("table", rows));
            continue;
        }
        if list_marker(trimmed).is_some() {
            flush(&mut paragraph, &mut out);
            let start = i;
            while i < lines.len() && list_marker(lines[i].trim()).is_some() {
                i += 1;
            }
            let items: Vec<(String, &str)> = lines[start..i]
                .iter()
                .filter_map(|l| list_marker(l.trim()))
                .collect();
            out.extend(build_lists(&items));
            continue;
        }
        paragraph.push(line);
        i += 1;
    }
    flush(&mut paragraph, &mut out);
    out
}

fn heading(line: &str) -> Option<Node> {
    let bytes = line.as_bytes();
    if bytes.len() >= 3 && bytes[0] == b'h' && (b'1'..=b'6').contains(&bytes[1]) && bytes[2] == b'.'
    {
        let level = (bytes[1] - b'0') as u64;
        return Some(Node {
            attrs: attrs(&[("level", json!(level))]),
            ..block("heading", parse_inline(line[3..].trim()))
        });
    }
    None
}

/// The markers (such as `*#`) and the text of a list line.
fn list_marker(line: &str) -> Option<(String, &str)> {
    let markers: String = line
        .chars()
        .take_while(|c| matches!(c, '*' | '#' | '-'))
        .collect();
    if markers.is_empty() {
        return None;
    }
    let rest = &line[markers.len()..];
    // `-` lists are one level only; `---` or `----` is not a list.
    if markers.contains('-') && markers.len() > 1 {
        return None;
    }
    rest.starts_with(' ').then(|| (markers, rest.trim_start()))
}

/// Nested lists from list lines, each with its markers.
fn build_lists(items: &[(String, &str)]) -> Vec<Node> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < items.len() {
        let depth_char = items[i].0.chars().next().unwrap_or('*');
        let ordered = depth_char == '#';
        let mut list = block(
            if ordered { "orderedList" } else { "bulletList" },
            Vec::new(),
        );
        // Items at this level, with deeper lines as their children.
        while i < items.len() && items[i].0.len() == 1 && items[i].0.starts_with(depth_char) {
            let mut item = block("listItem", vec![Node::paragraph(parse_inline(items[i].1))]);
            i += 1;
            let child_start = i;
            while i < items.len() && items[i].0.len() > 1 {
                i += 1;
            }
            if child_start < i {
                let children: Vec<(String, &str)> = items[child_start..i]
                    .iter()
                    .map(|(m, t)| (m[1..].to_string(), *t))
                    .collect();
                item.content.extend(build_lists(&children));
            }
            list.content.push(item);
        }
        if list.content.is_empty() {
            // Deeper lines with no parent: treat them as this level.
            let lifted: Vec<(String, &str)> =
                vec![(items[i].0[items[i].0.len() - 1..].to_string(), items[i].1)];
            out.extend(build_lists(&lifted));
            i += 1;
        } else {
            out.push(list);
        }
    }
    out
}

/// Splits on `|` outside `[...]` and `{...}`.
fn split_cells(line: &str) -> Vec<(bool, String)> {
    let mut cells = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    let mut depth = 0i32;
    let mut current = String::new();
    let mut header = false;
    let mut started = false;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '[' | '{' => depth += 1,
            ']' | '}' => depth -= 1,
            _ => {}
        }
        if c == '|' && depth <= 0 {
            if started {
                cells.push((header, current.trim().to_string()));
                current.clear();
            }
            started = true;
            header = i + 1 < chars.len() && chars[i + 1] == '|';
            if header {
                i += 1;
            }
            i += 1;
            continue;
        }
        current.push(c);
        i += 1;
    }
    if !current.trim().is_empty() {
        cells.push((header, current.trim().to_string()));
    }
    cells
}

fn table_row(line: &str) -> Node {
    let cells = split_cells(line)
        .into_iter()
        .map(|(header, text)| {
            block(
                if header { "tableHeader" } else { "tableCell" },
                vec![Node::paragraph(parse_inline(&text))],
            )
        })
        .collect();
    block("tableRow", cells)
}

fn param(params: &str, name: &str) -> Option<String> {
    params.split('|').find_map(|p| {
        let (k, v) = p.split_once('=')?;
        (k.trim().eq_ignore_ascii_case(name)).then(|| v.trim().to_string())
    })
}

fn macro_block(name: &str, params: &str, body: &[String]) -> Node {
    match name {
        "code" | "noformat" => {
            let text = body.join("\n").trim_matches('\n').to_string();
            let mut node = block("codeBlock", Vec::new());
            if !text.is_empty() {
                node.content.push(Node::text(text));
            }
            if name == "code" {
                let language = param(params, "language").or_else(|| {
                    params
                        .split('|')
                        .next()
                        .filter(|p| !p.contains('=') && !p.trim().is_empty())
                        .map(|p| p.trim().to_string())
                });
                if let Some(language) = language {
                    node.attrs = attrs(&[("language", json!(language.to_lowercase()))]);
                }
            }
            node
        }
        "quote" => {
            let lines: Vec<&str> = body.iter().map(String::as_str).collect();
            block("blockquote", non_empty(parse_blocks(&lines)))
        }
        _ => {
            let kind = match name {
                "note" => "note",
                "warning" => "warning",
                "tip" => "success",
                _ => "info",
            };
            let lines: Vec<&str> = body.iter().map(String::as_str).collect();
            let mut content = parse_blocks(&lines);
            if let Some(title) = param(params, "title") {
                content.insert(
                    0,
                    Node::paragraph(vec![Node {
                        marks: vec![simple("strong")],
                        ..Node::text(title)
                    }]),
                );
            }
            Node {
                attrs: attrs(&[("panelType", json!(kind))]),
                ..block("panel", non_empty(content))
            }
        }
    }
}

fn non_empty(mut content: Vec<Node>) -> Vec<Node> {
    if content.is_empty() {
        content.push(Node::paragraph(Vec::new()));
    }
    content
}

fn simple(kind: &str) -> Mark {
    Mark {
        kind: kind.into(),
        attrs: None,
    }
}

/// Inline marks, by their wiki markup character.
fn mark_for(c: char) -> Option<Mark> {
    Some(match c {
        '*' => simple("strong"),
        '_' => simple("em"),
        '-' => simple("strike"),
        '+' => simple("underline"),
        '^' => Mark {
            kind: "subsup".into(),
            attrs: attrs(&[("type", json!("sup"))]),
        },
        '~' => Mark {
            kind: "subsup".into(),
            attrs: attrs(&[("type", json!("sub"))]),
        },
        _ => return None,
    })
}

fn is_boundary(c: Option<char>) -> bool {
    c.is_none_or(|c| !c.is_alphanumeric())
}

/// Converts inline wiki markup.
pub fn parse_inline(text: &str) -> Vec<Node> {
    let mut out = Vec::new();
    inline_into(text, &[], &mut out);
    merge(out)
}

fn push_text(out: &mut Vec<Node>, text: &str, marks: &[Mark]) {
    if !text.is_empty() {
        out.push(Node {
            marks: marks.to_vec(),
            ..Node::text(text)
        });
    }
}

fn with(marks: &[Mark], mark: Mark) -> Vec<Mark> {
    let mut next = marks.to_vec();
    if !next.contains(&mark) {
        next.push(mark);
    }
    next
}

fn inline_into(text: &str, marks: &[Mark], out: &mut Vec<Node>) {
    let chars: Vec<char> = text.chars().collect();
    let mut plain = String::new();
    let mut i = 0;
    let flush = |plain: &mut String, out: &mut Vec<Node>| {
        push_text(out, plain, marks);
        plain.clear();
    };
    let rest = |from: usize| -> String { chars[from..].iter().collect() };
    while i < chars.len() {
        let c = chars[i];
        let prev = if i == 0 { None } else { Some(chars[i - 1]) };
        // Line breaks: `\\` and newlines inside a paragraph.
        if c == '\\' && chars.get(i + 1) == Some(&'\\') {
            flush(&mut plain, out);
            out.push(Node::hard_break());
            i += 2;
            continue;
        }
        if c == '\n' {
            flush(&mut plain, out);
            out.push(Node::hard_break());
            i += 1;
            continue;
        }
        // {{monospace}}
        if c == '{' && chars.get(i + 1) == Some(&'{') {
            let tail = rest(i + 2);
            if let Some(end) = tail.find("}}") {
                flush(&mut plain, out);
                push_text(out, &tail[..end], &with(marks, simple("code")));
                i += 2 + tail[..end].chars().count() + 2;
                continue;
            }
        }
        // {color:red}text{color}
        if c == '{' {
            let tail = rest(i);
            let lower = tail.to_lowercase();
            if lower.starts_with("{color:")
                && let Some(open_end) = tail.find('}')
                && let Some(close) = lower[open_end..].find("{color}")
            {
                let color = tail[7..open_end].trim().to_string();
                let inner = &tail[open_end + 1..open_end + close];
                flush(&mut plain, out);
                let mark = Mark {
                    kind: "textColor".into(),
                    attrs: attrs(&[("color", json!(color))]),
                };
                inline_into(inner, &with(marks, mark), out);
                i += tail[..open_end + close + 7].chars().count();
                continue;
            }
        }
        // [text|url], [url], [~accountid:id], [^attachment]
        if c == '[' {
            let tail = rest(i + 1);
            if let Some(end) = tail.find(']') {
                let inside = &tail[..end];
                let used = 1 + inside.chars().count() + 1;
                if let Some(user) = inside.strip_prefix('~') {
                    flush(&mut plain, out);
                    let id = user.strip_prefix("accountid:").unwrap_or(user);
                    out.push(Node {
                        attrs: attrs(&[("id", json!(id)), ("text", json!(""))]),
                        ..Node::new("mention")
                    });
                    i += used;
                    continue;
                }
                let (label, target) = match inside.rsplit_once('|') {
                    Some((label, target)) => (label.to_string(), target.trim().to_string()),
                    None => (inside.to_string(), inside.trim().to_string()),
                };
                let target = target.trim_start_matches('^').to_string();
                if !target.is_empty() {
                    flush(&mut plain, out);
                    let mark = Mark {
                        kind: "link".into(),
                        attrs: attrs(&[("href", json!(target))]),
                    };
                    let label = label.trim_start_matches('^');
                    inline_into(label, &with(marks, mark), out);
                    i += used;
                    continue;
                }
            }
        }
        // !image.png! and !image.png|thumbnail!
        if c == '!' && is_boundary(prev) {
            let tail = rest(i + 1);
            if let Some(end) = tail.find('!') {
                let inside = &tail[..end];
                let source = inside.split('|').next().unwrap_or_default().trim();
                if !source.is_empty()
                    && !source.contains(char::is_whitespace)
                    && source.contains('.')
                {
                    flush(&mut plain, out);
                    out.push(Node::image(source, ""));
                    i += 1 + inside.chars().count() + 1;
                    continue;
                }
            }
        }
        // ??citation??
        if c == '?' && chars.get(i + 1) == Some(&'?') && is_boundary(prev) {
            let tail = rest(i + 2);
            if let Some(end) = tail.find("??") {
                flush(&mut plain, out);
                inline_into(&tail[..end], &with(marks, simple("em")), out);
                i += 2 + tail[..end].chars().count() + 2;
                continue;
            }
        }
        // *bold* and friends: opened at a word start, closed before a word end.
        if let Some(mark) = mark_for(c)
            && is_boundary(prev)
            && chars
                .get(i + 1)
                .is_some_and(|n| !n.is_whitespace() && *n != c)
            && let Some(close) = (i + 1..chars.len()).find(|&j| {
                chars[j] == c
                    && !chars[j - 1].is_whitespace()
                    && is_boundary(chars.get(j + 1).copied())
            })
        {
            flush(&mut plain, out);
            let inner: String = chars[i + 1..close].iter().collect();
            inline_into(&inner, &with(marks, mark), out);
            i = close + 1;
            continue;
        }
        // Bare web addresses.
        if (c == 'h') && is_boundary(prev) {
            let tail = rest(i);
            if tail.starts_with("http://") || tail.starts_with("https://") {
                let url: String = tail
                    .chars()
                    .take_while(|c| !c.is_whitespace() && !matches!(c, '|' | ']' | ')' | '"'))
                    .collect();
                let url = url.trim_end_matches(['.', ',', ';', ':']).to_string();
                flush(&mut plain, out);
                let mark = Mark {
                    kind: "link".into(),
                    attrs: attrs(&[("href", json!(url))]),
                };
                push_text(out, &url, &with(marks, mark));
                i += url.chars().count();
                continue;
            }
        }
        plain.push(c);
        i += 1;
    }
    flush(&mut plain, out);
}

/// Joins neighbouring text with the same marks.
fn merge(nodes: Vec<Node>) -> Vec<Node> {
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
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(markup: &str) -> Value {
        serde_json::to_value(from_wiki_markup(markup)).unwrap()
    }

    #[test]
    fn blocks() {
        let doc = json(
            "h2. Plan\n\nFirst line\nsecond line\n\n* one\n** inner\n* two\n# first\n\n----\n||Head||Other||\n|a|[link|https://e.com]|\nbq. quoted",
        );
        let content = doc["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "heading");
        assert_eq!(content[0]["attrs"]["level"], 2);
        assert_eq!(content[1]["content"][1]["type"], "hardBreak");
        assert_eq!(content[2]["type"], "bulletList");
        assert_eq!(content[2]["content"][0]["content"][1]["type"], "bulletList");
        assert_eq!(content[2]["content"].as_array().unwrap().len(), 2);
        assert_eq!(content[3]["type"], "orderedList");
        assert_eq!(content[4]["type"], "rule");
        assert_eq!(
            content[5]["content"][0]["content"][0]["type"],
            "tableHeader"
        );
        assert_eq!(
            content[5]["content"][1]["content"][1]["content"][0]["content"][0]["marks"][0]["attrs"]
                ["href"],
            "https://e.com"
        );
        assert_eq!(content[6]["type"], "blockquote");
    }

    #[test]
    fn macros() {
        let doc = json(
            "{code:java}\nint a = 1;\n{code}\n{noformat}raw *text*{noformat}\n{panel:title=Heads up}\nCareful\n{panel}\n{quote}\nwise words\n{quote}",
        );
        let content = doc["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "codeBlock");
        assert_eq!(content[0]["attrs"]["language"], "java");
        assert_eq!(content[0]["content"][0]["text"], "int a = 1;");
        assert_eq!(content[1]["content"][0]["text"], "raw *text*");
        assert_eq!(content[2]["type"], "panel");
        assert_eq!(content[2]["content"][0]["content"][0]["text"], "Heads up");
        assert_eq!(content[3]["type"], "blockquote");
    }

    #[test]
    fn inline() {
        let nodes = parse_inline(
            "A *bold* _it_ -gone- {{code}} [~accountid:557058:abc] see https://example.com. well-known *not closed",
        );
        let json = serde_json::to_value(&nodes).unwrap();
        let find = |text: &str| {
            json.as_array()
                .unwrap()
                .iter()
                .find(|n| n["text"] == text)
                .cloned()
                .unwrap_or(Value::Null)
        };
        assert_eq!(find("bold")["marks"][0]["type"], "strong");
        assert_eq!(find("it")["marks"][0]["type"], "em");
        assert_eq!(find("gone")["marks"][0]["type"], "strike");
        assert_eq!(find("code")["marks"][0]["type"], "code");
        assert_eq!(find("https://example.com")["marks"][0]["type"], "link");
        let mention = json
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["type"] == "mention")
            .unwrap();
        assert_eq!(mention["attrs"]["id"], "557058:abc");
        // Hyphens inside words and unclosed marks stay as text.
        let all: String = json
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|n| n["text"].as_str())
            .collect();
        assert!(all.contains("well-known"));
        assert!(all.contains("*not closed"));
    }
}
