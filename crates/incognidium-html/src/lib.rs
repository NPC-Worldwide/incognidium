use html5ever::parse_document;
use html5ever::tendril::TendrilSink;
use html5ever::tree_builder::TreeBuilderOpts;
use html5ever::{Attribute, ParseOpts, QualName};
use std::borrow::Cow;
use std::cell::UnsafeCell;

use incognidium_dom::{Document, ElementData, NodeData, NodeId, TextData};

/// Interior-mutable storage for the tree sink.
/// html5ever's TreeSink trait takes &self for mutating methods,
/// so we need interior mutability.
struct SinkData {
    doc: Document,
    qual_names: Vec<Option<QualName>>,
    quirks_mode: html5ever::tree_builder::QuirksMode,
}

struct DomSink {
    data: UnsafeCell<SinkData>,
}

impl DomSink {
    fn data(&self) -> &mut SinkData {
        unsafe { &mut *self.data.get() }
    }
}

#[derive(Clone, Debug)]
struct Handle(NodeId);

impl PartialEq for Handle {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl html5ever::tree_builder::TreeSink for DomSink {
    type Handle = Handle;
    type Output = Document;
    type ElemName<'a> = html5ever::ExpandedName<'a>;

    fn finish(self) -> Self::Output {
        self.data.into_inner().doc
    }

    fn parse_error(&self, _msg: Cow<'static, str>) {}

    fn get_document(&self) -> Handle {
        Handle(0)
    }

    fn elem_name<'a>(&'a self, target: &'a Handle) -> html5ever::ExpandedName<'a> {
        let d = self.data();
        d.qual_names[target.0]
            .as_ref()
            .expect("elem_name called on non-element")
            .expanded()
    }

    fn create_element(
        &self,
        name: QualName,
        attrs: Vec<Attribute>,
        _flags: html5ever::tree_builder::ElementFlags,
    ) -> Handle {
        let d = self.data();
        let mut el = ElementData::new(name.local.to_string());
        for attr in attrs {
            el.attributes
                .insert(attr.name.local.to_string(), attr.value.to_string());
        }
        let id = d.doc.nodes.len();
        d.doc.nodes.push(incognidium_dom::Node {
            id,
            parent: None,
            children: Vec::new(),
            data: NodeData::Element(el),
        });
        while d.qual_names.len() <= id {
            d.qual_names.push(None);
        }
        d.qual_names[id] = Some(name);
        Handle(id)
    }

    fn create_comment(&self, text: html5ever::tendril::StrTendril) -> Handle {
        let d = self.data();
        let id = d.doc.nodes.len();
        d.doc.nodes.push(incognidium_dom::Node {
            id,
            parent: None,
            children: Vec::new(),
            data: NodeData::Comment(text.to_string()),
        });
        while d.qual_names.len() <= id {
            d.qual_names.push(None);
        }
        Handle(id)
    }

    fn create_pi(
        &self,
        _target: html5ever::tendril::StrTendril,
        _data: html5ever::tendril::StrTendril,
    ) -> Handle {
        let d = self.data();
        let id = d.doc.nodes.len();
        d.doc.nodes.push(incognidium_dom::Node {
            id,
            parent: None,
            children: Vec::new(),
            data: NodeData::Comment(String::new()),
        });
        while d.qual_names.len() <= id {
            d.qual_names.push(None);
        }
        Handle(id)
    }

    fn append(&self, parent: &Handle, child: html5ever::tree_builder::NodeOrText<Handle>) {
        let d = self.data();
        match child {
            html5ever::tree_builder::NodeOrText::AppendNode(handle) => {
                if let Some(old_parent) = d.doc.nodes[handle.0].parent {
                    d.doc.nodes[old_parent]
                        .children
                        .retain(|&id| id != handle.0);
                }
                d.doc.nodes[handle.0].parent = Some(parent.0);
                d.doc.nodes[parent.0].children.push(handle.0);
            }
            html5ever::tree_builder::NodeOrText::AppendText(text) => {
                if let Some(&last_id) = d.doc.nodes[parent.0].children.last() {
                    if let NodeData::Text(ref mut t) = d.doc.nodes[last_id].data {
                        t.content.push_str(&text);
                        return;
                    }
                }
                let id = d.doc.nodes.len();
                d.doc.nodes.push(incognidium_dom::Node {
                    id,
                    parent: Some(parent.0),
                    children: Vec::new(),
                    data: NodeData::Text(TextData {
                        content: text.to_string(),
                    }),
                });
                while d.qual_names.len() <= id {
                    d.qual_names.push(None);
                }
                d.doc.nodes[parent.0].children.push(id);
            }
        }
    }

    fn append_based_on_parent_node(
        &self,
        element: &Handle,
        _prev_element: &Handle,
        child: html5ever::tree_builder::NodeOrText<Handle>,
    ) {
        self.append(element, child);
    }

    fn append_doctype_to_document(
        &self,
        _name: html5ever::tendril::StrTendril,
        _public_id: html5ever::tendril::StrTendril,
        _system_id: html5ever::tendril::StrTendril,
    ) {
    }

    fn get_template_contents(&self, target: &Handle) -> Handle {
        target.clone()
    }

    fn same_node(&self, x: &Handle, y: &Handle) -> bool {
        x.0 == y.0
    }

    fn set_quirks_mode(&self, mode: html5ever::tree_builder::QuirksMode) {
        self.data().quirks_mode = mode;
    }

    fn append_before_sibling(
        &self,
        sibling: &Handle,
        new_node: html5ever::tree_builder::NodeOrText<Handle>,
    ) {
        let d = self.data();
        let parent_id = match d.doc.nodes[sibling.0].parent {
            Some(p) => p,
            None => return,
        };

        match new_node {
            html5ever::tree_builder::NodeOrText::AppendNode(handle) => {
                if let Some(old_parent) = d.doc.nodes[handle.0].parent {
                    d.doc.nodes[old_parent]
                        .children
                        .retain(|&id| id != handle.0);
                }
                d.doc.nodes[handle.0].parent = Some(parent_id);
                let siblings = &mut d.doc.nodes[parent_id].children;
                if let Some(pos) = siblings.iter().position(|&id| id == sibling.0) {
                    siblings.insert(pos, handle.0);
                } else {
                    siblings.push(handle.0);
                }
            }
            html5ever::tree_builder::NodeOrText::AppendText(text) => {
                let id = d.doc.nodes.len();
                d.doc.nodes.push(incognidium_dom::Node {
                    id,
                    parent: Some(parent_id),
                    children: Vec::new(),
                    data: NodeData::Text(TextData {
                        content: text.to_string(),
                    }),
                });
                while d.qual_names.len() <= id {
                    d.qual_names.push(None);
                }
                let siblings = &mut d.doc.nodes[parent_id].children;
                if let Some(pos) = siblings.iter().position(|&id2| id2 == sibling.0) {
                    siblings.insert(pos, id);
                } else {
                    siblings.push(id);
                }
            }
        }
    }

    fn add_attrs_if_missing(&self, target: &Handle, attrs: Vec<Attribute>) {
        let d = self.data();
        if let NodeData::Element(ref mut el) = d.doc.nodes[target.0].data {
            for attr in attrs {
                el.attributes
                    .entry(attr.name.local.to_string())
                    .or_insert_with(|| attr.value.to_string());
            }
        }
    }

    fn remove_from_parent(&self, target: &Handle) {
        let d = self.data();
        if let Some(parent_id) = d.doc.nodes[target.0].parent {
            d.doc.nodes[parent_id]
                .children
                .retain(|&id| id != target.0);
            d.doc.nodes[target.0].parent = None;
        }
    }

    fn reparent_children(&self, node: &Handle, new_parent: &Handle) {
        let d = self.data();
        let children: Vec<NodeId> = d.doc.nodes[node.0].children.clone();
        d.doc.nodes[node.0].children.clear();
        for child_id in children {
            d.doc.nodes[child_id].parent = Some(new_parent.0);
            d.doc.nodes[new_parent.0].children.push(child_id);
        }
    }
}

/// Parse an HTML string into a DOM Document.
pub fn parse_html(html: &str) -> Document {
    let sink = DomSink {
        data: UnsafeCell::new(SinkData {
            doc: Document::new(),
            qual_names: vec![None],
            quirks_mode: html5ever::tree_builder::QuirksMode::NoQuirks,
        }),
    };
    let opts = ParseOpts {
        tree_builder: TreeBuilderOpts {
            drop_doctype: true,
            scripting_enabled: false,
            ..Default::default()
        },
        ..Default::default()
    };
    parse_document(sink, opts)
        .from_utf8()
        .one(html.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_html() {
        let doc = parse_html("<html><body><p>Hello</p></body></html>");
        assert!(doc.document_element().is_some());
        assert!(doc.body().is_some());
        println!("{}", doc);
    }

    #[test]
    fn test_parse_with_style() {
        let html = r#"
            <html>
            <head><style>p { color: red; }</style></head>
            <body><p>Styled text</p></body>
            </html>
        "#;
        let doc = parse_html(html);
        let css = doc.collect_style_text();
        assert!(css.contains("color: red"));
    }

    #[test]
    fn test_parse_attributes() {
        let doc = parse_html(r#"<html><body><div id="main" class="container flex">Content</div></body></html>"#);
        let main = doc.get_element_by_id("main");
        assert!(main.is_some());
        if let NodeData::Element(ref el) = doc.node(main.unwrap()).data {
            assert_eq!(el.classes(), vec!["container", "flex"]);
        }
    }
}
