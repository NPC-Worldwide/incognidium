use std::collections::HashMap;
use std::fmt;

/// Unique identifier for a DOM node within a document.
pub type NodeId = usize;

/// The DOM tree, stored as a flat arena for cache-friendly access.
#[derive(Debug, Default, Clone)]
pub struct Document {
    pub nodes: Vec<Node>,
    /// URL fragment identifier (without the leading `#`) that identifies the
    /// element matched by the CSS `:target` pseudo-class.
    pub target_id: Option<String>,
}

impl Document {
    pub fn new() -> Self {
        let mut doc = Document {
            nodes: Vec::new(),
            target_id: None,
        };
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

    /// Repair malformed parent/child pointers that can be introduced by JS DOM
    /// manipulation (e.g. reparenting the document root or creating cycles).
    /// Keeps only the tree reachable from the real document root and removes
    /// duplicate/cyclic child references.
    pub fn sanitize_tree(&mut self) {
        if self.nodes.is_empty() {
            return;
        }
        self.nodes[0].parent = None;
        let mut visited: std::collections::HashSet<NodeId> = std::collections::HashSet::new();
        let mut stack: Vec<NodeId> = vec![0];
        while let Some(node_id) = stack.pop() {
            if !visited.insert(node_id) {
                continue;
            }
            let mut new_children = Vec::new();
            {
                let node = &self.nodes[node_id];
                for &child_id in &node.children {
                    if child_id == 0 || child_id >= self.nodes.len() || visited.contains(&child_id)
                    {
                        continue;
                    }
                    new_children.push(child_id);
                }
            }
            for &child_id in &new_children {
                self.nodes[child_id].parent = Some(node_id);
                stack.push(child_id);
            }
            self.nodes[node_id].children = new_children;
        }
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
        for node in &self.nodes {
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
        }
        css
    }

    /// Collect all <noscript> elements' text content (server-rendered fallback).
    pub fn collect_noscript_text(&self) -> String {
        let mut text = String::new();
        for node in &self.nodes {
            if let NodeData::Element(ref el) = node.data {
                if el.tag_name == "noscript" {
                    for &child_id in &node.children {
                        if let NodeData::Text(ref t) = self.nodes[child_id].data {
                            text.push_str(&t.content);
                            text.push('\n');
                        }
                    }
                }
            }
        }
        text
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

/// Event listener entry for DOM events
#[derive(Debug, Clone)]
pub struct EventListener {
    pub event_type: String,
    pub handler: String, // JavaScript code as string for now
    pub capture: bool,
}

#[derive(Debug, Clone)]
pub struct ElementData {
    pub tag_name: String,
    pub attributes: HashMap<String, String>,
    pub event_listeners: Vec<EventListener>,
}

impl ElementData {
    pub fn new(tag_name: impl Into<String>) -> Self {
        ElementData {
            tag_name: tag_name.into(),
            attributes: HashMap::new(),
            event_listeners: Vec::new(),
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
