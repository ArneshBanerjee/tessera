//! The rich-text editor: what you see is what you get, for pages, descriptions, and comments.
//!
//! It edits ADF. The document is drawn as editable HTML ([`tessera_adf::editor`]), and every
//! change is read back from the page into ADF. Typing `@` offers people to mention.

use leptos::html;
use leptos::prelude::*;
use tessera_adf::Node;
use tessera_adf::editor::{DomKind, DomNode, from_editor, to_editor_html};
use wasm_bindgen::JsCast;

use crate::components::{Avatar, users_resource};
use crate::model::User;

/// A node of the live page, for reading the editor back.
struct Live(web_sys::Node);

impl DomNode for Live {
    fn kind(&self) -> DomKind {
        match self.0.node_type() {
            web_sys::Node::TEXT_NODE => DomKind::Text(self.0.text_content().unwrap_or_default()),
            web_sys::Node::ELEMENT_NODE => DomKind::Element,
            _ => DomKind::Other,
        }
    }

    fn tag(&self) -> String {
        self.0.node_name().to_lowercase()
    }

    fn attr(&self, name: &str) -> Option<String> {
        self.0.dyn_ref::<web_sys::Element>()?.get_attribute(name)
    }

    fn children(&self) -> Vec<Self> {
        let list = self.0.child_nodes();
        (0..list.length())
            .filter_map(|i| list.item(i))
            .map(Live)
            .collect()
    }

    fn checked(&self) -> bool {
        self.0
            .dyn_ref::<web_sys::HtmlInputElement>()
            .is_some_and(web_sys::HtmlInputElement::checked)
    }
}

fn document() -> Option<web_sys::HtmlDocument> {
    web_sys::window()?.document()?.dyn_into().ok()
}

/// Runs a browser editing command on the selection.
fn exec(command: &str, value: &str) {
    if let Some(document) = document() {
        let _ = document.exec_command_with_show_ui_and_value(command, false, value);
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The `@name` typed just before the caret: its text node, where the `@` is, where the caret
/// is, and the name so far.
fn mention_at_caret() -> Option<(web_sys::Node, u32, u32, String)> {
    let selection = web_sys::window()?.get_selection().ok()??;
    if !selection.is_collapsed() {
        return None;
    }
    let node = selection.anchor_node()?;
    if node.node_type() != web_sys::Node::TEXT_NODE {
        return None;
    }
    let caret = selection.anchor_offset();
    let text = node.text_content().unwrap_or_default();
    let units: Vec<u16> = text.encode_utf16().collect();
    let before = String::from_utf16_lossy(&units[..(caret as usize).min(units.len())]);
    let at = before.rfind('@')?;
    let typed = &before[at + 1..];
    let starts_word = before[..at].chars().last().is_none_or(char::is_whitespace);
    let plausible = typed.chars().count() <= 30 && !typed.contains('\n') && !typed.starts_with(' ');
    (starts_word && plausible).then(|| {
        let at_units = before[..at].encode_utf16().count() as u32;
        (node, at_units, caret, typed.to_string())
    })
}

/// Swaps the typed `@name` for a mention of `user`, and puts the caret after it.
fn insert_mention(user: &User) {
    let Some((node, at, caret, _)) = mention_at_caret() else {
        return;
    };
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };
    let Ok(range) = document.create_range() else {
        return;
    };
    if range.set_start(&node, at).is_err() || range.set_end(&node, caret).is_err() {
        return;
    }
    let _ = range.delete_contents();
    let Ok(span) = document.create_element("span") else {
        return;
    };
    let _ = span.set_attribute("class", "mention");
    let _ = span.set_attribute("contenteditable", "false");
    let _ = span.set_attribute("data-mention-id", &user.account_id);
    span.set_text_content(Some(&format!("@{}", user.display_name)));
    let space = document.create_text_node("\u{a0}");
    let _ = range.insert_node(&space);
    let _ = range.insert_node(&span);
    if let Ok(Some(selection)) = window.get_selection() {
        let _ = range.set_start_after(&space);
        range.collapse_with_to_start(true);
        let _ = selection.remove_all_ranges();
        let _ = selection.add_range(&range);
    }
}

/// HTML for new blocks the toolbar adds. `data-caret` marks where typing goes next.
const TASK_LIST: &str = "<ul data-task-list=\"true\"><li data-task=\"TODO\" data-caret=\"\"><input type=\"checkbox\" contenteditable=\"false\">&#8203;</li></ul><p><br></p>";
const PANEL: &str = "<div class=\"editor-panel panel-info\" data-panel-type=\"info\"><p data-caret=\"\">&#8203;</p></div><p><br></p>";
const TABLE: &str = "<table><tbody><tr><th><p data-caret=\"\">&#8203;</p></th><th><p><br></p></th><th><p><br></p></th></tr><tr><td><p><br></p></td><td><p><br></p></td><td><p><br></p></td></tr></tbody></table><p><br></p>";

/// Inserts a block, and puts the caret where it is marked.
fn insert_block(surface: &web_sys::Element, html: &str) {
    exec("insertHTML", html);
    let Ok(Some(target)) = surface.query_selector("[data-caret]") else {
        return;
    };
    let _ = target.remove_attribute("data-caret");
    let (Some(window), Some(document)) = (
        web_sys::window(),
        web_sys::window().and_then(|w| w.document()),
    ) else {
        return;
    };
    if let (Ok(range), Ok(Some(selection))) = (document.create_range(), window.get_selection()) {
        let _ = range.select_node_contents(&target);
        range.collapse_with_to_start(false);
        let _ = selection.remove_all_ranges();
        let _ = selection.add_range(&range);
    }
}

/// A rich-text editor for one document. `on_change` gets the document after every change.
/// `compact` gives a smaller toolbar, for comments.
#[component]
pub fn RichEditor(
    initial: Node,
    on_change: Callback<Node>,
    #[prop(optional)] compact: bool,
    #[prop(optional)] autofocus: bool,
    #[prop(into, optional)] placeholder: String,
    #[prop(into, optional)] label: String,
) -> impl IntoView {
    let surface = NodeRef::<html::Div>::new();
    let users = users_resource();
    let query = RwSignal::new(None::<String>);
    let active = RwSignal::new(0usize);
    let menu_at = RwSignal::new((0.0, 0.0));
    let initial_html = to_editor_html(&initial);
    let read = move || {
        if let Some(element) = surface.get_untracked() {
            let node: &web_sys::Node = element.as_ref();
            on_change.run(from_editor(&Live(node.clone())));
        }
    };
    Effect::new(move |_| {
        if let Some(element) = surface.get() {
            // New lines become paragraphs rather than divs.
            exec("defaultParagraphSeparator", "p");
            if autofocus {
                let _ = element.focus();
            }
        }
    });
    let matches = move || -> Vec<User> {
        let Some(typed) = query.get() else {
            return Vec::new();
        };
        let typed = typed.to_lowercase();
        users
            .get()
            .unwrap_or_default()
            .into_iter()
            .filter(|u| u.display_name.to_lowercase().contains(&typed))
            .take(6)
            .collect()
    };
    let update_query = move || match mention_at_caret() {
        Some((_, _, _, typed)) => {
            if let Some(frame) = surface.get_untracked()
                && let Ok(Some(selection)) = web_sys::window()
                    .map(|w| w.get_selection())
                    .transpose()
                    .map(Option::flatten)
                && let Ok(range) = selection.get_range_at(0)
            {
                let rect = range.get_bounding_client_rect();
                let outer = frame.get_bounding_client_rect();
                menu_at.set((
                    rect.bottom() - outer.top() + 4.0,
                    rect.left() - outer.left(),
                ));
            }
            query.set(Some(typed));
        }
        None => query.set(None),
    };
    let pick = move |user: User| {
        insert_mention(&user);
        query.set(None);
        read();
    };
    let on_key = move |ev: leptos::ev::KeyboardEvent| {
        let list = matches();
        if list.is_empty() {
            return;
        }
        match ev.key().as_str() {
            "ArrowDown" => {
                ev.prevent_default();
                active.update(|a| *a = (*a + 1) % list.len());
            }
            "ArrowUp" => {
                ev.prevent_default();
                active.update(|a| *a = (*a + list.len() - 1) % list.len());
            }
            "Enter" | "Tab" => {
                ev.prevent_default();
                pick(list[active.get_untracked().min(list.len() - 1)].clone());
            }
            "Escape" => query.set(None),
            _ => {}
        }
    };
    // Pasting keeps the text and drops outside formatting.
    let on_paste = move |ev: web_sys::ClipboardEvent| {
        let Some(clip) = ev.clipboard_data() else {
            return;
        };
        if let Ok(text) = clip.get_data("text/plain") {
            ev.prevent_default();
            exec("insertText", &text);
            read();
        }
    };
    let block_button = move |html: &'static str| {
        move |ev: leptos::ev::MouseEvent| {
            ev.prevent_default();
            if let Some(element) = surface.get_untracked() {
                insert_block(&element, html);
                read();
            }
        }
    };
    // Toolbar buttons act on mouse down, so the editor keeps its selection.
    let command = move |cmd: &'static str, value: &'static str| {
        move |ev: leptos::ev::MouseEvent| {
            ev.prevent_default();
            exec(cmd, value);
            read();
        }
    };
    let link = move |ev: leptos::ev::MouseEvent| {
        ev.prevent_default();
        let url = web_sys::window()
            .and_then(|w| w.prompt_with_message("Link to (https://…)").ok().flatten())
            .filter(|u| !u.trim().is_empty());
        if let Some(url) = url {
            exec("createLink", url.trim());
            read();
        }
    };
    let inline_code = move |ev: leptos::ev::MouseEvent| {
        ev.prevent_default();
        let selected = web_sys::window()
            .and_then(|w| w.get_selection().ok().flatten())
            .map(|s| String::from(s.to_string()))
            .unwrap_or_default();
        if !selected.is_empty() {
            exec("insertHTML", &format!("<code>{}</code>", escape(&selected)));
            read();
        }
    };
    let mention = move |ev: leptos::ev::MouseEvent| {
        ev.prevent_default();
        exec("insertText", "@");
        update_query();
    };
    let block = move |value: String| {
        exec("formatBlock", &value);
        read();
    };
    view! {
        <div class="rich-editor-frame" class:compact=compact>
            <div class="rich-toolbar" role="toolbar" aria-label="Formatting">
                {(!compact).then(|| view! {
                    <select class="select rich-block" aria-label="Text style"
                        on:mousedown=|ev| ev.stop_propagation()
                        on:change:target=move |ev| { block(ev.target().value()); ev.target().set_value(""); }>
                        <option value="">"Text style"</option>
                        <option value="<p>">"Normal text"</option>
                        <option value="<h1>">"Heading 1"</option>
                        <option value="<h2>">"Heading 2"</option>
                        <option value="<h3>">"Heading 3"</option>
                        <option value="<blockquote>">"Quote"</option>
                        <option value="<pre>">"Code block"</option>
                    </select>
                })}
                <button class="rich-button" title="Bold" aria-label="Bold" on:mousedown=command("bold", "")><strong>"B"</strong></button>
                <button class="rich-button" title="Italic" aria-label="Italic" on:mousedown=command("italic", "")><em>"I"</em></button>
                <button class="rich-button" title="Underline" aria-label="Underline" on:mousedown=command("underline", "")><u>"U"</u></button>
                <button class="rich-button" title="Strikethrough" aria-label="Strikethrough" on:mousedown=command("strikeThrough", "")><s>"S"</s></button>
                <button class="rich-button" title="Code" aria-label="Code" on:mousedown=inline_code>"</>"</button>
                <button class="rich-button" title="Link" aria-label="Link" on:mousedown=link>"🔗"</button>
                <span class="rich-sep"></span>
                <button class="rich-button" title="Bullet list" aria-label="Bullet list" on:mousedown=command("insertUnorderedList", "")>"•≡"</button>
                <button class="rich-button" title="Numbered list" aria-label="Numbered list" on:mousedown=command("insertOrderedList", "")>"1≡"</button>
                <button class="rich-button" title="Task list" aria-label="Task list" on:mousedown=block_button(TASK_LIST)>"☑"</button>
                {(!compact).then(|| view! {
                    <button class="rich-button" title="Info panel" aria-label="Info panel" on:mousedown=block_button(PANEL)>"ⓘ"</button>
                    <button class="rich-button" title="Table" aria-label="Table" on:mousedown=block_button(TABLE)>"▦"</button>
                    <button class="rich-button" title="Divider" aria-label="Divider" on:mousedown=command("insertHorizontalRule", "")>"—"</button>
                })}
                <button class="rich-button" title="Mention someone" aria-label="Mention someone" on:mousedown=mention>"@"</button>
                <span class="rich-sep"></span>
                <button class="rich-button" title="Undo" aria-label="Undo" on:mousedown=command("undo", "")>"↶"</button>
                <button class="rich-button" title="Redo" aria-label="Redo" on:mousedown=command("redo", "")>"↷"</button>
            </div>
            <div class="rich-editor wiki-body" contenteditable="true" role="textbox" aria-multiline="true"
                aria-label=if label.is_empty() { "Editor".to_string() } else { label }
                data-placeholder=placeholder
                node_ref=surface inner_html=initial_html
                on:input=move |_| { read(); update_query(); }
                on:keyup=move |_| update_query()
                on:keydown=on_key
                on:paste=on_paste
                // Ticking a task box changes the document without an input event.
                on:click=move |ev: leptos::ev::MouseEvent| {
                    if ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()).is_some() {
                        read();
                    }
                }
                on:blur=move |_| query.set(None)></div>
            {move || {
                let list = matches();
                (!list.is_empty()).then(|| {
                    let (top, left) = menu_at.get();
                    view! {
                        <ul class="mention-menu" role="listbox" aria-label="People to mention"
                            style=format!("top: {top}px; left: {left}px")>
                            {list.into_iter().enumerate().map(|(i, user)| {
                                let chosen = user.clone();
                                view! {
                                    <li role="option" class="mention-option" class:active=move || active.get() == i
                                        on:mousedown=move |ev| { ev.prevent_default(); pick(chosen.clone()); }>
                                        <Avatar user=Some(user.clone()) size=20/>
                                        <span>{user.display_name.clone()}</span>
                                    </li>
                                }
                            }).collect_view()}
                        </ul>
                    }
                })
            }}
        </div>
    }
}
