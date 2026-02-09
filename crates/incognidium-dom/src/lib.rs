use std::collections::HashMap;
use std::fmt;

/// Unique identifier for a DOM node within a document.
pub type NodeId = usize;

/// The DOM tree, stored as a flat arena for cache-friendly access.
#[derive(Debug, Default)]
pub struct Document {
    pub nodes: Vec<Node>,
}

impl Document {
    pub fn new() -> Self {
        let mut doc = Document { nodes: Vec::new() };
        // Node 0 is always the Document node
        doc.nodes.push(Node {
            id: 0,
            parent: None,
            children: Vec::new(),
            data: NodeData::Document,
        });
        doc
    }

    pub fn root(&self) -> NodeId {
        0
    }

    pub fn add_node(&mut self, parent: NodeId, data: NodeData) -> NodeId {
        let id = self.nodes.len();
        self.nodes.push(Node {
            id,
            parent: Some(parent),
            children: Vec::new(),
            data,
        });
        self.nodes[parent].children.push(id);
        id
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id]
    }

    pub fn node_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id]
    }

    /// Find the <html> element (usually the first element child of root).
    pub fn document_element(&self) -> Option<NodeId> {
        self.nodes[0].children.iter().copied().find(|&id| {
            matches!(
                &self.nodes[id].data,
                NodeData::Element(ref e) if e.tag_name == "html"
            )
        })
    }

    /// Find <body> by walking children of <html>.
    pub fn body(&self) -> Option<NodeId> {
        let html = self.document_element()?;
        self.nodes[html].children.iter().copied().find(|&id| {
            matches!(
                &self.nodes[id].data,
                NodeData::Element(ref e) if e.tag_name == "body"
            )
        })
    }

    /// Collect all <style> elements' text content.
    pub fn collect_style_text(&self) -> String {
        let mut css = String::new();
        self.collect_style_text_recursive(self.root(), &mut css);
        css
    }

    fn collect_style_text_recursive(&self, node_id: NodeId, css: &mut String) {
        let node = &self.nodes[node_id];
        if let NodeData::Element(ref el) = node.data {
            if el.tag_name == "style" {
                for &child_id in &node.children {
                    if let NodeData::Text(ref t) = self.nodes[child_id].data {
                        css.push_str(&t.content);
                        css.push('\n');
                    }
                }
            }
        }
        for &child_id in &node.children.clone() {
            self.collect_style_text_recursive(child_id, css);
        }
    }

    /// Get element by id attribute.
    pub fn get_element_by_id(&self, id: &str) -> Option<NodeId> {
        self.nodes.iter().find_map(|node| {
            if let NodeData::Element(ref el) = node.data {
                if el.attributes.get("id").map(|v| v.as_str()) == Some(id) {
                    return Some(node.id);
                }
            }
            None
        })
    }
}

#[derive(Debug, Clone)]
pub struct Node {
    pub id: NodeId,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    pub data: NodeData,
}

#[derive(Debug, Clone)]
pub enum NodeData {
    Document,
    Element(ElementData),
    Text(TextData),
    Comment(String),
}

#[derive(Debug, Clone)]
pub struct ElementData {
    pub tag_name: String,
    pub attributes: HashMap<String, String>,
}

impl ElementData {
    pub fn new(tag_name: impl Into<String>) -> Self {
        ElementData {
            tag_name: tag_name.into(),
            attributes: HashMap::new(),
        }
    }

    pub fn classes(&self) -> Vec<&str> {
        self.attributes
            .get("class")
            .map(|c| c.split_whitespace().collect())
            .unwrap_or_default()
    }

    pub fn id(&self) -> Option<&str> {
        self.attributes.get("id").map(|s| s.as_str())
    }

    pub fn get_attr(&self, name: &str) -> Option<&str> {
        self.attributes.get(name).map(|s| s.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct TextData {
    pub content: String,
}

impl fmt::Display for Document {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt_node(f, 0, 0)
    }
}

impl Document {
    fn fmt_node(&self, f: &mut fmt::Formatter<'_>, node_id: NodeId, depth: usize) -> fmt::Result {
        let indent = "  ".repeat(depth);
        let node = &self.nodes[node_id];
        match &node.data {
            NodeData::Document => {
                writeln!(f, "{indent}#document")?;
            }
            NodeData::Element(el) => {
                write!(f, "{indent}<{}", el.tag_name)?;
                for (k, v) in &el.attributes {
                    write!(f, " {k}=\"{v}\"")?;
                }
                writeln!(f, ">")?;
            }
            NodeData::Text(t) => {
                let text = t.content.trim();
                if !text.is_empty() {
                    writeln!(f, "{indent}\"{text}\"")?;
                }
            }
            NodeData::Comment(c) => {
                writeln!(f, "{indent}<!-- {c} -->")?;
            }
        }
        for &child in &node.children {
            self.fmt_node(f, child, depth + 1)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_dom() {
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));
        let _p = doc.add_node(body, NodeData::Element(ElementData::new("p")));
        let _text = doc.add_node(
            _p,
            NodeData::Text(TextData {
                content: "Hello, world!".to_string(),
            }),
        );

        assert_eq!(doc.nodes.len(), 5);
        assert_eq!(doc.document_element(), Some(html));
        assert_eq!(doc.body(), Some(body));
    }

    #[test]
    fn test_element_classes() {
        let mut el = ElementData::new("div");
        el.attributes
            .insert("class".to_string(), "foo bar baz".to_string());
        assert_eq!(el.classes(), vec!["foo", "bar", "baz"]);
    }
}
