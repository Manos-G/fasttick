//! Node types of the document arena.

use std::borrow::Cow;
use std::fmt;

use super::escape::{decode, escape_attribute, is_all_whitespace};

/// The identifier of a node in a [`Document`](super::Document).
///
/// A `NodeId` is an index into the arena of one document. It stays valid
/// while the document exists, also when the node is detached. Do not use a
/// `NodeId` with a different document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub(crate) u32);

impl NodeId {
    /// Returns the arena index of the node.
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// The quote character of an attribute value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Quote {
    /// The value is between double quotes (`"`).
    #[default]
    Double,
    /// The value is between single quotes (`'`).
    Single,
}

impl Quote {
    /// Returns the quote character.
    pub fn as_char(self) -> char {
        match self {
            Quote::Double => '"',
            Quote::Single => '\'',
        }
    }

    /// Returns the quote for a character, if it is `"` or `'`.
    pub fn from_char(c: char) -> Option<Quote> {
        match c {
            '"' => Some(Quote::Double),
            '\'' => Some(Quote::Single),
            _ => None,
        }
    }
}

/// One attribute of an element.
///
/// An attribute keeps its source text, so that the writer gives the same
/// bytes when the attribute did not change.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Attribute {
    pub(crate) name: String,
    pub(crate) value: String,
    pub(crate) raw: String,
    pub(crate) quote: Quote,
    pub(crate) leading: String,
    pub(crate) equals: String,
}

impl Attribute {
    /// Makes a new attribute with a single space before the name, `=`
    /// without spaces and double quotes.
    ///
    /// The value is escaped. This function does not check the name; the
    /// editing functions of [`Document`](super::Document) do.
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Attribute {
        Attribute::with_style(name, value, Quote::Double, " ")
    }

    /// Makes a new attribute with the given quote and leading white space.
    pub(crate) fn with_style(
        name: impl Into<String>,
        value: impl Into<String>,
        quote: Quote,
        leading: &str,
    ) -> Attribute {
        let value = value.into();
        let raw = escape_attribute(&value, quote.as_char());
        Attribute {
            name: name.into(),
            value,
            raw,
            quote,
            leading: leading.to_owned(),
            equals: "=".to_owned(),
        }
    }

    /// Makes an attribute from source text. `raw` is the text between the
    /// quotes.
    pub(crate) fn from_source(
        name: &str,
        raw: &str,
        quote: Quote,
        leading: &str,
        equals: &str,
    ) -> Attribute {
        Attribute {
            name: name.to_owned(),
            value: decode(raw, true).into_owned(),
            raw: raw.to_owned(),
            quote,
            leading: leading.to_owned(),
            equals: equals.to_owned(),
        }
    }

    /// Changes the value. The quote, the leading white space and the text
    /// around `=` stay the same.
    pub(crate) fn set_value(&mut self, value: &str) {
        self.raw = escape_attribute(value, self.quote.as_char());
        self.value = value.to_owned();
    }

    /// Returns the attribute name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the decoded value, as BT.CPP (tinyxml2) sees it.
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Returns the value text between the quotes, exactly as in the source.
    pub fn raw_value(&self) -> &str {
        &self.raw
    }

    /// Returns the quote character.
    pub fn quote(&self) -> Quote {
        self.quote
    }

    /// Returns the white space before the attribute name.
    pub fn leading_whitespace(&self) -> &str {
        &self.leading
    }

    /// Returns the text between the name and the opening quote, for example
    /// `=` or ` = `.
    pub fn equals_text(&self) -> &str {
        &self.equals
    }

    /// Writes the attribute as it appears in a start tag, with the leading
    /// white space.
    pub(crate) fn write_to(&self, out: &mut String) {
        let q = self.quote.as_char();
        out.push_str(&self.leading);
        out.push_str(&self.name);
        out.push_str(&self.equals);
        out.push(q);
        out.push_str(&self.raw);
        out.push(q);
    }
}

/// An element node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    pub(crate) name: String,
    pub(crate) attributes: Vec<Attribute>,
    pub(crate) self_closing: bool,
    /// White space between the last attribute (or the name) and `>` or `/>`.
    pub(crate) tail: String,
    /// The start tag as in the source. Empty for a new element.
    pub(crate) raw_start: String,
    /// White space between the name and `>` in the end tag.
    pub(crate) end_ws: String,
    /// True when the name or the attributes changed after parsing.
    pub(crate) dirty: bool,
}

impl Element {
    /// Makes a new element without attributes. It is written as `<name/>`
    /// while it has no children.
    pub(crate) fn new(name: &str) -> Element {
        Element {
            name: name.to_owned(),
            attributes: Vec::new(),
            self_closing: true,
            tail: String::new(),
            raw_start: String::new(),
            end_ws: String::new(),
            dirty: true,
        }
    }

    /// Returns the element name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the attributes in source order.
    pub fn attributes(&self) -> &[Attribute] {
        &self.attributes
    }

    /// Returns the decoded value of an attribute.
    pub fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.value.as_str())
    }

    /// Returns the position of an attribute in [`Element::attributes`].
    pub fn attribute_index(&self, name: &str) -> Option<usize> {
        self.attributes.iter().position(|a| a.name == name)
    }

    /// Returns true if the source used an empty-element tag (`<a/>`).
    ///
    /// A new element also returns true. The writer uses `<a/>` only while
    /// the element has no children.
    pub fn is_self_closing(&self) -> bool {
        self.self_closing
    }

    /// Returns true if the name or the attributes changed after parsing.
    /// The writer then makes a new start tag.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Returns the start tag text from the source, or `None` for a new
    /// element.
    pub fn raw_start_tag(&self) -> Option<&str> {
        if self.raw_start.is_empty() {
            None
        } else {
            Some(&self.raw_start)
        }
    }

    /// Returns the white space before `>` (or `/>`) in the start tag.
    pub fn tail_whitespace(&self) -> &str {
        &self.tail
    }

    /// Returns the white space before `>` in the end tag.
    pub fn end_tag_whitespace(&self) -> &str {
        &self.end_ws
    }

    /// Writes a new start tag from the parts. `self_close` selects `/>`.
    pub(crate) fn generate_start(&self, self_close: bool, out: &mut String) {
        out.push('<');
        out.push_str(&self.name);
        for (i, a) in self.attributes.iter().enumerate() {
            // The first attribute needs white space after the name.
            if i == 0 && a.leading.is_empty() {
                out.push(' ');
            }
            a.write_to(out);
        }
        if self_close {
            out.push_str(&self.tail);
            out.push_str("/>");
        } else {
            // An empty-element tag that now has children: the white space
            // before `/>` (usually one space) is not useful before `>`.
            if !self.self_closing {
                out.push_str(&self.tail);
            }
            out.push('>');
        }
    }

    /// Writes the start tag. It uses the source text when possible.
    pub(crate) fn write_start(&self, has_children: bool, out: &mut String) {
        let self_close = self.self_closing && !has_children;
        if !self.dirty && !self.raw_start.is_empty() && self_close == self.self_closing {
            out.push_str(&self.raw_start);
        } else {
            self.generate_start(self_close, out);
        }
    }

    /// Writes the end tag.
    pub(crate) fn write_end(&self, out: &mut String) {
        out.push_str("</");
        out.push_str(&self.name);
        out.push_str(&self.end_ws);
        out.push('>');
    }
}

/// A text node. It keeps the source text with its entity references.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Text {
    pub(crate) raw: String,
}

impl Text {
    /// Returns the text as in the source, with entity references.
    pub fn raw(&self) -> &str {
        &self.raw
    }

    /// Returns the decoded text, as BT.CPP (tinyxml2) sees it.
    pub fn decoded(&self) -> Cow<'_, str> {
        decode(&self.raw, true)
    }

    /// Returns true if the text has only white space (or is empty).
    pub fn is_whitespace(&self) -> bool {
        is_all_whitespace(&self.raw)
    }
}

/// A CDATA section, `<![CDATA[...]]>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CData {
    pub(crate) raw: String,
}

impl CData {
    /// Returns the full source text, with `<![CDATA[` and `]]>`.
    pub fn raw(&self) -> &str {
        &self.raw
    }

    /// Returns the text between `<![CDATA[` and `]]>`.
    pub fn content(&self) -> &str {
        self.raw.get(9..self.raw.len().saturating_sub(3)).unwrap_or("")
    }

    /// Returns the content with line ends normalized as tinyxml2 does.
    pub fn decoded(&self) -> Cow<'_, str> {
        decode(self.content(), false)
    }
}

/// A comment, `<!--...-->`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    pub(crate) raw: String,
}

impl Comment {
    /// Returns the full source text, with `<!--` and `-->`.
    pub fn raw(&self) -> &str {
        &self.raw
    }

    /// Returns the text between `<!--` and `-->`.
    pub fn content(&self) -> &str {
        self.raw.get(4..self.raw.len().saturating_sub(3)).unwrap_or("")
    }
}

/// A processing instruction, `<?target data?>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessingInstruction {
    pub(crate) raw: String,
}

impl ProcessingInstruction {
    /// Returns the full source text, with `<?` and `?>`.
    pub fn raw(&self) -> &str {
        &self.raw
    }

    /// Returns the text between `<?` and `?>`.
    pub fn content(&self) -> &str {
        self.raw.get(2..self.raw.len().saturating_sub(2)).unwrap_or("")
    }

    /// Returns the target name.
    pub fn target(&self) -> &str {
        let c = self.content();
        let end = c
            .find(|ch: char| ch.is_ascii_whitespace())
            .unwrap_or(c.len());
        &c[..end]
    }

    /// Returns the data after the target and the white space.
    pub fn data(&self) -> &str {
        let c = self.content();
        c[self.target().len()..].trim_start_matches([' ', '\t', '\r', '\n'])
    }
}

/// The XML declaration, `<?xml version="1.0"?>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declaration {
    pub(crate) raw: String,
}

impl Declaration {
    /// Returns the full source text, with `<?` and `?>`.
    pub fn raw(&self) -> &str {
        &self.raw
    }

    /// Returns the text between `<?xml` and `?>`.
    pub fn content(&self) -> &str {
        self.raw.get(5..self.raw.len().saturating_sub(2)).unwrap_or("")
    }

    /// Returns the value of a pseudo-attribute such as `version` or
    /// `encoding`, if it is present and well-formed.
    pub fn pseudo_attribute(&self, name: &str) -> Option<&str> {
        let mut rest = self.content();
        loop {
            rest = rest.trim_start_matches([' ', '\t', '\r', '\n']);
            let eq = rest.find('=')?;
            let key = rest[..eq].trim_end_matches([' ', '\t', '\r', '\n']);
            rest = rest[eq + 1..].trim_start_matches([' ', '\t', '\r', '\n']);
            let q = rest.chars().next()?;
            if q != '"' && q != '\'' {
                return None;
            }
            let close = rest[1..].find(q)? + 1;
            let value = &rest[1..close];
            if key == name {
                return Some(value);
            }
            rest = &rest[close + 1..];
        }
    }
}

/// A document type declaration, `<!DOCTYPE ...>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocType {
    pub(crate) raw: String,
}

impl DocType {
    /// Returns the full source text, with `<!DOCTYPE` and `>`.
    pub fn raw(&self) -> &str {
        &self.raw
    }

    /// Returns the text after `<!DOCTYPE` and before the final `>`, without
    /// the leading white space.
    pub fn content(&self) -> &str {
        self.raw
            .get(9..self.raw.len().saturating_sub(1))
            .unwrap_or("")
            .trim_start_matches([' ', '\t', '\r', '\n'])
    }
}

/// The kind of a node, without its data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NodeKind {
    /// The document node. It is the parent of the top-level items.
    Document,
    /// An element.
    Element,
    /// Character data with entity references.
    Text,
    /// A CDATA section.
    CData,
    /// A comment.
    Comment,
    /// A processing instruction.
    ProcessingInstruction,
    /// The XML declaration.
    Declaration,
    /// A document type declaration.
    DocType,
}

/// The data of one node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    /// The document node. Each document has exactly one, at
    /// [`Document::root`](super::Document::root).
    Document,
    /// An element.
    Element(Element),
    /// Character data with entity references.
    Text(Text),
    /// A CDATA section.
    CData(CData),
    /// A comment.
    Comment(Comment),
    /// A processing instruction.
    ProcessingInstruction(ProcessingInstruction),
    /// The XML declaration.
    Declaration(Declaration),
    /// A document type declaration.
    DocType(DocType),
}

impl Node {
    /// Returns the kind of the node.
    pub fn kind(&self) -> NodeKind {
        match self {
            Node::Document => NodeKind::Document,
            Node::Element(_) => NodeKind::Element,
            Node::Text(_) => NodeKind::Text,
            Node::CData(_) => NodeKind::CData,
            Node::Comment(_) => NodeKind::Comment,
            Node::ProcessingInstruction(_) => NodeKind::ProcessingInstruction,
            Node::Declaration(_) => NodeKind::Declaration,
            Node::DocType(_) => NodeKind::DocType,
        }
    }

    /// Returns the element data, if this is an element.
    pub fn as_element(&self) -> Option<&Element> {
        match self {
            Node::Element(e) => Some(e),
            _ => None,
        }
    }

    /// Returns the text data, if this is a text node.
    pub fn as_text(&self) -> Option<&Text> {
        match self {
            Node::Text(t) => Some(t),
            _ => None,
        }
    }

    /// Returns the source text of a node that is not an element or the
    /// document node.
    pub fn raw(&self) -> Option<&str> {
        match self {
            Node::Document | Node::Element(_) => None,
            Node::Text(t) => Some(&t.raw),
            Node::CData(c) => Some(&c.raw),
            Node::Comment(c) => Some(&c.raw),
            Node::ProcessingInstruction(p) => Some(&p.raw),
            Node::Declaration(d) => Some(&d.raw),
            Node::DocType(d) => Some(&d.raw),
        }
    }
}

/// One slot of the arena.
#[derive(Debug, Clone)]
pub(crate) struct Slot {
    pub(crate) parent: Option<NodeId>,
    pub(crate) children: Vec<NodeId>,
    pub(crate) node: Node,
    pub(crate) offset: Option<usize>,
}

impl Slot {
    pub(crate) fn new(node: Node, offset: Option<usize>) -> Slot {
        Slot {
            parent: None,
            children: Vec::new(),
            node,
            offset,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declaration_pseudo_attributes() {
        let d = Declaration {
            raw: "<?xml version=\"1.0\" encoding='UTF-8' ?>".to_owned(),
        };
        assert_eq!(d.pseudo_attribute("version"), Some("1.0"));
        assert_eq!(d.pseudo_attribute("encoding"), Some("UTF-8"));
        assert_eq!(d.pseudo_attribute("standalone"), None);
    }

    #[test]
    fn pi_target_and_data() {
        let p = ProcessingInstruction {
            raw: "<?target  some data ?>".to_owned(),
        };
        assert_eq!(p.target(), "target");
        assert_eq!(p.data(), "some data ");
    }

    #[test]
    fn content_accessors() {
        let c = Comment {
            raw: "<!-- hi -->".to_owned(),
        };
        assert_eq!(c.content(), " hi ");
        let c = CData {
            raw: "<![CDATA[a<b]]>".to_owned(),
        };
        assert_eq!(c.content(), "a<b");
        let d = DocType {
            raw: "<!DOCTYPE  root [<!ENTITY a \"b\">]>".to_owned(),
        };
        assert_eq!(d.content(), "root [<!ENTITY a \"b\">]");
    }
}
