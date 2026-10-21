//! Streaming the records of a backup's `entities.xml`, one at a time.
//!
//! - Jira (OfBiz): `<entity-engine-xml>` holds one element per record, named for its kind,
//!   with fields as attributes or, for long text, as child elements.
//! - Confluence (Hibernate): `<hibernate-generic>` holds `<object class="Page">` records with
//!   `<id name="id">`, `<property name="title">`, and references such as
//!   `<property name="space" class="Space"><id name="id">7</id></property>`.

use std::collections::HashMap;
use std::io::BufRead;

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};

use super::{ImportError, ImportResult};

/// One record: its kind and its fields, as text.
#[derive(Debug, Clone, Default)]
pub struct Entity {
    pub kind: String,
    pub fields: HashMap<String, String>,
}

impl Entity {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.fields
            .get(name)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    }

    pub fn text(&self, name: &str) -> String {
        self.get(name).unwrap_or_default().to_string()
    }

    pub fn int(&self, name: &str) -> Option<i64> {
        self.get(name)?.trim().parse().ok()
    }
}

/// A small element tree for one record.
#[derive(Debug, Default)]
struct Element {
    name: String,
    attrs: HashMap<String, String>,
    children: Vec<Element>,
    text: String,
}

fn xml_error(error: impl std::fmt::Display) -> ImportError {
    ImportError::Xml(error.to_string())
}

fn start_element(start: &BytesStart) -> ImportResult<Element> {
    let name = start.name().as_ref().to_string();
    let mut attrs = HashMap::new();
    for attr in start.attributes() {
        let attr = attr.map_err(xml_error)?;
        let key = attr.key.as_ref().to_string();
        let value = attr
            .normalized_value(XmlVersion::Implicit1_0)
            .map_err(xml_error)?
            .to_string();
        attrs.insert(key, value);
    }
    Ok(Element {
        name,
        attrs,
        ..Default::default()
    })
}

/// Text for an entity or character reference such as `amp` or `#x41`.
fn resolve_reference(name: &str) -> String {
    if let Some(code) = name.strip_prefix("#x").or_else(|| name.strip_prefix("#X")) {
        return u32::from_str_radix(code, 16)
            .ok()
            .and_then(char::from_u32)
            .map(String::from)
            .unwrap_or_default();
    }
    if let Some(code) = name.strip_prefix('#') {
        return code
            .parse::<u32>()
            .ok()
            .and_then(char::from_u32)
            .map(String::from)
            .unwrap_or_default();
    }
    match name {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" => "'",
        _ => "",
    }
    .to_string()
}

/// Reads every record under the root, calling `handle` for those whose kind `wanted` accepts.
/// `kind_of` names a record's kind from its top element.
fn read_records(
    source: impl BufRead,
    kind_of: impl Fn(&Element) -> String,
    wanted: impl Fn(&str) -> bool,
    mut handle: impl FnMut(&Element) -> ImportResult<()>,
) -> ImportResult<()> {
    let mut reader = Reader::from_reader(source);
    let mut buf = Vec::new();
    // The open elements: the root, then the record being built and its descendants.
    let mut stack: Vec<Element> = Vec::new();
    let mut depth = 0usize;
    let mut skipping = false;
    loop {
        let event = reader.read_event_into(&mut buf).map_err(xml_error)?;
        match event {
            Event::Start(start) => {
                depth += 1;
                if depth == 1 || skipping {
                    buf.clear();
                    continue;
                }
                let element = start_element(&start)?;
                if depth == 2 && !wanted(&kind_of(&element)) {
                    skipping = true;
                    buf.clear();
                    continue;
                }
                stack.push(element);
            }
            Event::Empty(start) => {
                if depth == 0 || skipping {
                    buf.clear();
                    continue;
                }
                let element = start_element(&start)?;
                if depth == 1 {
                    if wanted(&kind_of(&element)) {
                        handle(&element)?;
                    }
                } else if let Some(parent) = stack.last_mut() {
                    parent.children.push(element);
                }
            }
            Event::End(_) => {
                if depth == 2 && skipping {
                    skipping = false;
                } else if depth >= 2 && !skipping {
                    let done = stack.pop().expect("an open element");
                    match stack.last_mut() {
                        Some(parent) => parent.children.push(done),
                        None => handle(&done)?,
                    }
                }
                depth = depth.saturating_sub(1);
            }
            Event::Text(text) => {
                if !skipping && let Some(open) = stack.last_mut() {
                    open.text.push_str(&text.xml10_content());
                }
            }
            Event::CData(data) => {
                if !skipping && let Some(open) = stack.last_mut() {
                    open.text.push_str(&data.into_inner());
                }
            }
            Event::GeneralRef(reference) => {
                if !skipping && let Some(open) = stack.last_mut() {
                    let name = reference.into_inner().to_string();
                    open.text.push_str(&resolve_reference(&name));
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(())
}

/// Reads Jira's OfBiz records of the kinds `wanted` accepts.
pub fn read_ofbiz(
    source: impl BufRead,
    wanted: impl Fn(&str) -> bool,
    mut handle: impl FnMut(Entity) -> ImportResult<()>,
) -> ImportResult<()> {
    read_records(
        source,
        |e| e.name.clone(),
        wanted,
        |element| {
            let mut fields = element.attrs.clone();
            for child in &element.children {
                fields.insert(child.name.clone(), child.text.clone());
            }
            handle(Entity {
                kind: element.name.clone(),
                fields,
            })
        },
    )
}

/// Reads Confluence's Hibernate objects of the classes `wanted` accepts. A reference to another
/// object becomes that object's id.
pub fn read_hibernate(
    source: impl BufRead,
    wanted: impl Fn(&str) -> bool,
    mut handle: impl FnMut(Entity) -> ImportResult<()>,
) -> ImportResult<()> {
    read_records(
        source,
        |e| e.attrs.get("class").cloned().unwrap_or_default(),
        wanted,
        |element| {
            let mut fields = HashMap::new();
            for child in &element.children {
                let Some(name) = child.attrs.get("name") else {
                    continue;
                };
                match child.name.as_str() {
                    "id" => {
                        fields.insert(name.clone(), child.text.trim().to_string());
                    }
                    "property" => {
                        let value = match child.children.iter().find(|c| c.name == "id") {
                            Some(id) => id.text.trim().to_string(),
                            None => child.text.clone(),
                        };
                        fields.insert(name.clone(), value);
                    }
                    _ => {}
                }
            }
            handle(Entity {
                kind: element.attrs.get("class").cloned().unwrap_or_default(),
                fields,
            })
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_ofbiz_records() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<entity-engine-xml>
  <Project id="10000" key="DEMO" name="Demo &amp; more"/>
  <Issue id="10001" project="10000" number="1" summary="Hi">
    <description><![CDATA[Some *bold* text]]></description>
  </Issue>
  <Skipped id="1"><child>x</child></Skipped>
</entity-engine-xml>"#;
        let mut found = Vec::new();
        read_ofbiz(
            xml.as_bytes(),
            |k| k != "Skipped",
            |e| {
                found.push(e);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].text("name"), "Demo & more");
        assert_eq!(found[1].text("description"), "Some *bold* text");
        assert_eq!(found[1].int("number"), Some(1));
    }

    #[test]
    fn reads_hibernate_objects() {
        let xml = r#"<hibernate-generic datetime="2024-01-01 00:00:00">
<object class="Page" package="com.atlassian.confluence.pages">
<id name="id">65537</id>
<property name="title"><![CDATA[Home & away]]></property>
<property name="space" class="Space" package="com.atlassian.confluence.spaces"><id name="id">98305</id></property>
<collection name="bodyContents" class="java.util.Collection"><element class="BodyContent"><id name="id">1</id></element></collection>
</object>
<object class="Other" package="x"><id name="id">1</id></object>
</hibernate-generic>"#;
        let mut found = Vec::new();
        read_hibernate(
            xml.as_bytes(),
            |k| k == "Page",
            |e| {
                found.push(e);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, "Page");
        assert_eq!(found[0].text("id"), "65537");
        assert_eq!(found[0].text("title"), "Home & away");
        assert_eq!(found[0].text("space"), "98305");
    }
}
