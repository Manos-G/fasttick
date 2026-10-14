//! The parser. It uses the quick-xml event reader to find the markup and
//! checks each tag with the rules of tinyxml2.

use quick_xml::Reader;
use quick_xml::errors::SyntaxError;
use quick_xml::events::Event;

use super::document::{DEFAULT_INDENT, Document, line_col_in};
use super::error::XmlError;
use super::escape::{is_all_whitespace, is_xml_whitespace, name_len};
use super::node::{
    Attribute, CData, Comment, Declaration, DocType, Element, Node, NodeId, ProcessingInstruction,
    Quote, Slot, Text,
};

/// The deepest element nesting that BT.CPP accepts.
///
/// tinyxml2 stops with `XML_ELEMENT_DEPTH_EXCEEDED` when its parse depth
/// reaches `TINYXML2_MAX_ELEMENT_DEPTH` (500). The document level counts as
/// one, and the content of each open element counts as one more
/// (`XMLDocument::PushDepth`, `XMLNode::ParseDeep`). An element with content
/// at nesting level 499 is therefore an error.
pub const MAX_ELEMENT_DEPTH: usize = 498;

/// What the parser reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// A full document.
    Document,
    /// A fragment: top-level text must be white space.
    Fragment,
}

/// An error position (byte offset in the full input) and a message.
type Fail = (usize, String);

impl Document {
    /// Parses a document.
    ///
    /// The parser accepts what tinyxml2 (the XML reader of BT.CPP) accepts.
    /// Like tinyxml2, it also accepts some text that is not well-formed XML:
    /// a `&` that does not start a reference, `<` in attribute values, more
    /// than one top-level element, text outside the document element, `--`
    /// in comments, `]]>` in text and attributes without white space between
    /// them.
    ///
    /// These are errors: an empty input, a malformed tag, a missing or wrong
    /// end tag, a duplicate attribute, an XML declaration after other
    /// content, unclosed comments, CDATA sections, processing instructions
    /// or DOCTYPE declarations, and elements nested deeper than
    /// [`MAX_ELEMENT_DEPTH`].
    ///
    /// Differences from tinyxml2:
    ///
    /// - Processing instructions are allowed everywhere. tinyxml2 reads each
    ///   `<?...?>` as a declaration and accepts it only at the start.
    /// - Markup that starts with `<!` and is not a comment, a CDATA section
    ///   or a DOCTYPE is an error. tinyxml2 keeps it as an "unknown" node.
    /// - A DOCTYPE with an internal subset is one node. tinyxml2 ends the
    ///   "unknown" node at the first `>` (`XMLDocument::Identify`,
    ///   `XMLUnknown::ParseDeep`).
    /// - Only space, tab, CR and LF are white space in tags. tinyxml2 also
    ///   accepts vertical tab and form feed.
    pub fn parse(text: &str) -> Result<Document, XmlError> {
        parse_document(text, Mode::Document)
    }

    /// Parses a document from bytes. The bytes must be UTF-8 (with or
    /// without a byte order mark).
    pub fn parse_bytes(bytes: &[u8]) -> Result<Document, XmlError> {
        if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
            return Err(XmlError {
                line: 1,
                column: 1,
                offset: 0,
                message: "the file is UTF-16; FastTick reads only UTF-8 XML files".to_owned(),
            });
        }
        match std::str::from_utf8(bytes) {
            Ok(text) => Document::parse(text),
            Err(e) => {
                let offset = e.valid_up_to();
                // The valid prefix is UTF-8, so the position is easy to find.
                let prefix = std::str::from_utf8(&bytes[..offset]).unwrap_or("");
                let doc = Document::with_source(prefix, false);
                let (line, column) = doc.line_col(offset);
                let byte = bytes.get(offset).copied().unwrap_or(0);
                Err(XmlError {
                    line,
                    column,
                    offset,
                    message: format!(
                        "the file is not valid UTF-8 (invalid byte 0x{byte:02X}); \
                         FastTick reads only UTF-8 XML files"
                    ),
                })
            }
        }
    }

    /// Parses a fragment with one or more top-level elements, and adds its
    /// nodes to this document as detached nodes.
    ///
    /// The result has the top-level elements, comments, processing
    /// instructions and CDATA sections of the fragment, in order. White
    /// space between them, XML declarations and DOCTYPE declarations are
    /// dropped. Text that is not white space is an error, and so is a
    /// fragment without an element. Error positions refer to `text`.
    pub fn parse_fragment(&mut self, text: &str) -> Result<Vec<NodeId>, XmlError> {
        let fragment = parse_document(text, Mode::Fragment)?;
        let tops: Vec<NodeId> = fragment
            .children(fragment.root())
            .iter()
            .copied()
            .filter(|&id| {
                !matches!(
                    fragment.node(id),
                    Some(Node::Text(_) | Node::Declaration(_) | Node::DocType(_))
                )
            })
            .collect();
        let mut out = Vec::with_capacity(tops.len());
        for id in tops {
            let copy = self.import_subtree(&fragment, id).map_err(|e| XmlError {
                line: 1,
                column: 1,
                offset: 0,
                message: e.to_string(),
            })?;
            out.push(copy);
        }
        Ok(out)
    }
}

/// Parses `text` into a new document.
pub(crate) fn parse_document(text: &str, mode: Mode) -> Result<Document, XmlError> {
    let bom = text.starts_with('\u{feff}');
    let base = if bom { '\u{feff}'.len_utf8() } else { 0 };
    let mut doc = Document::with_source(text, bom);
    let body = &text[base..];
    let result = if is_all_whitespace(body) {
        // tinyxml2: XML_ERROR_EMPTY_DOCUMENT (XMLDocument::Parse).
        Err((
            base,
            if mode == Mode::Fragment {
                "the fragment contains no element".to_owned()
            } else {
                "the document is empty".to_owned()
            },
        ))
    } else {
        Builder::new(&mut doc, body, base, mode).run()
    };
    match result {
        Ok(()) => {
            doc.indent_unit = infer_indent_unit(&doc);
            Ok(doc)
        }
        Err((offset, message)) => {
            let (line, column) = line_col_in(text, &doc.line_starts, offset);
            Err(XmlError {
                line,
                column,
                offset,
                message,
            })
        }
    }
}

/// Builds the arena from the reader events.
struct Builder<'d, 't> {
    doc: &'d mut Document,
    body: &'t str,
    base: usize,
    mode: Mode,
    /// Open elements. The first item is the document node.
    stack: Vec<NodeId>,
    /// The text node that the previous event wrote to, if any.
    open_text: Option<NodeId>,
}

impl<'d, 't> Builder<'d, 't> {
    fn new(doc: &'d mut Document, body: &'t str, base: usize, mode: Mode) -> Self {
        let root = doc.root();
        Builder {
            doc,
            body,
            base,
            mode,
            stack: vec![root],
            open_text: None,
        }
    }

    fn parent(&self) -> NodeId {
        self.stack.last().copied().unwrap_or(NodeId(0))
    }

    /// Adds a node as the last child of the current parent.
    fn push(&mut self, node: Node, offset: usize) -> Result<NodeId, Fail> {
        let parent = self.parent();
        let index = self.doc.slots.len();
        let id = u32::try_from(index)
            .map(NodeId)
            .map_err(|_| (offset, "the document has too many nodes".to_owned()))?;
        let mut slot = Slot::new(node, Some(offset));
        slot.parent = Some(parent);
        self.doc.slots.push(slot);
        if let Some(p) = self.doc.slots.get_mut(parent.index()) {
            p.children.push(id);
        }
        Ok(id)
    }

    fn run(mut self) -> Result<(), Fail> {
        let mut reader = Reader::from_str(self.body);
        {
            let config = reader.config_mut();
            // tinyxml2 keeps a lone `&` as text.
            config.allow_dangling_amp = true;
            // The builder checks the end tags itself.
            config.allow_unmatched_ends = true;
            config.check_end_names = false;
            config.check_comments = false;
            config.expand_empty_elements = false;
            config.trim_text(false);
        }
        loop {
            let start = usize::try_from(reader.buffer_position()).unwrap_or(usize::MAX);
            let event = match reader.read_event() {
                Ok(event) => event,
                Err(e) => {
                    let at = usize::try_from(reader.error_position()).unwrap_or(0);
                    // quick-xml reports an unclosed tag at its `<`. A wrong
                    // quote is often the real cause; find it.
                    if let quick_xml::Error::Syntax(
                        SyntaxError::UnclosedTag
                        | SyntaxError::UnclosedSingleQuotedAttributeValue
                        | SyntaxError::UnclosedDoubleQuotedAttributeValue,
                    ) = e
                        && let Some((rel, message)) =
                            self.body.get(at..).and_then(diagnose_unclosed_tag)
                    {
                        return Err((self.base + at + rel, message));
                    }
                    return Err((self.base + at, e.to_string()));
                }
            };
            let end = usize::try_from(reader.buffer_position()).unwrap_or(usize::MAX);
            let raw = self.body.get(start..end).unwrap_or("");
            let offset = self.base + start;
            if let Event::Text(_) | Event::GeneralRef(_) = event {
                self.text(raw, offset)?;
                continue;
            }
            self.open_text = None;
            match event {
                Event::Start(_) => self.start_tag(raw, offset, false)?,
                Event::Empty(_) => self.start_tag(raw, offset, true)?,
                Event::End(_) => self.end_tag(raw, offset)?,
                Event::CData(_) => {
                    self.push(
                        Node::CData(CData {
                            raw: raw.to_owned(),
                        }),
                        offset,
                    )?;
                }
                Event::Comment(_) => {
                    self.push(
                        Node::Comment(Comment {
                            raw: raw.to_owned(),
                        }),
                        offset,
                    )?;
                }
                Event::Decl(_) => self.declaration(raw, offset)?,
                Event::PI(_) => self.processing_instruction(raw, offset)?,
                Event::DocType(_) => {
                    self.push(
                        Node::DocType(DocType {
                            raw: raw.to_owned(),
                        }),
                        offset,
                    )?;
                }
                Event::Eof => return self.finish(),
                Event::Text(_) | Event::GeneralRef(_) => {}
            }
        }
    }

    fn text(&mut self, raw: &str, offset: usize) -> Result<(), Fail> {
        if let Some(id) = self.open_text {
            if let Some(Node::Text(t)) = self.doc.slots.get_mut(id.index()).map(|s| &mut s.node) {
                t.raw.push_str(raw);
            }
            return Ok(());
        }
        let id = self.push(
            Node::Text(Text {
                raw: raw.to_owned(),
            }),
            offset,
        )?;
        self.open_text = Some(id);
        Ok(())
    }

    fn start_tag(&mut self, raw: &str, offset: usize, empty: bool) -> Result<(), Fail> {
        let tag = parse_start_tag(raw).map_err(|(rel, msg)| (offset + rel, msg))?;
        if tag.self_closing != empty {
            return Err((offset, "malformed start tag".to_owned()));
        }
        if !empty && self.stack.len() > MAX_ELEMENT_DEPTH {
            return Err((
                offset,
                format!(
                    "the elements are nested too deeply: BT.CPP reads at most \
                     {MAX_ELEMENT_DEPTH} levels of elements with content"
                ),
            ));
        }
        let element = Element {
            name: tag.name.to_owned(),
            attributes: tag.attributes,
            self_closing: empty,
            tail: tag.tail.to_owned(),
            raw_start: raw.to_owned(),
            end_ws: String::new(),
            dirty: false,
        };
        let id = self.push(Node::Element(element), offset)?;
        if !empty {
            self.stack.push(id);
        }
        Ok(())
    }

    fn end_tag(&mut self, raw: &str, offset: usize) -> Result<(), Fail> {
        let (name, ws) = parse_end_tag(raw).map_err(|(rel, msg)| (offset + rel, msg))?;
        if self.stack.len() < 2 {
            return Err((
                offset,
                format!("the end tag </{name}> has no matching start tag"),
            ));
        }
        let id = self.parent();
        let (open_name, open_offset) = match self.doc.slots.get(id.index()) {
            Some(Slot {
                node: Node::Element(e),
                offset,
                ..
            }) => (e.name.clone(), offset.unwrap_or(0)),
            _ => (String::new(), 0),
        };
        if open_name != name {
            let (line, column) = line_col_in(&self.doc.source, &self.doc.line_starts, open_offset);
            return Err((
                offset,
                format!(
                    "the end tag </{name}> does not match the start tag <{open_name}> \
                     at line {line}, column {column}"
                ),
            ));
        }
        if let Some(Node::Element(e)) = self.doc.slots.get_mut(id.index()).map(|s| &mut s.node) {
            e.end_ws = ws.to_owned();
        }
        self.stack.pop();
        Ok(())
    }

    fn declaration(&mut self, raw: &str, offset: usize) -> Result<(), Fail> {
        // tinyxml2 allows declarations only at the document level, before
        // all other nodes (XMLNode::ParseDeep, "wellLocated"). It skips the
        // white space at the document level, so white space can come first.
        let well_located = self.stack.len() == 1
            && self
                .doc
                .children(self.doc.root())
                .iter()
                .all(|&c| match self.doc.node(c) {
                    Some(Node::Declaration(_)) => true,
                    Some(Node::Text(t)) => t.is_whitespace(),
                    _ => false,
                });
        if !well_located {
            return Err((
                offset,
                "the XML declaration must be at the start of the document".to_owned(),
            ));
        }
        self.push(
            Node::Declaration(Declaration {
                raw: raw.to_owned(),
            }),
            offset,
        )?;
        Ok(())
    }

    fn processing_instruction(&mut self, raw: &str, offset: usize) -> Result<(), Fail> {
        let content = raw.get(2..raw.len().saturating_sub(2)).unwrap_or("");
        let target_len = name_len(content);
        if target_len == 0 {
            return Err((
                offset + 2,
                "a processing instruction must start with a target name".to_owned(),
            ));
        }
        if content
            .as_bytes()
            .get(target_len)
            .is_some_and(|&b| !is_xml_whitespace(b))
        {
            return Err((
                offset + 2 + target_len,
                "expected white space after the processing instruction target".to_owned(),
            ));
        }
        self.push(
            Node::ProcessingInstruction(ProcessingInstruction {
                raw: raw.to_owned(),
            }),
            offset,
        )?;
        Ok(())
    }

    fn finish(self) -> Result<(), Fail> {
        if self.stack.len() > 1 {
            let id = self.parent();
            let (name, offset) = match self.doc.slots.get(id.index()) {
                Some(Slot {
                    node: Node::Element(e),
                    offset,
                    ..
                }) => (e.name.clone(), offset.unwrap_or(0)),
                _ => (String::new(), 0),
            };
            return Err((offset, format!("the element <{name}> has no end tag")));
        }
        let root = self.doc.root();
        let tops = self.doc.children(root).to_vec();
        if self.mode == Mode::Fragment {
            for id in &tops {
                if let Some(Node::Text(t)) = self.doc.node(*id)
                    && !t.is_whitespace()
                {
                    let offset = self.doc.source_offset(*id).unwrap_or(0);
                    let skip =
                        t.raw.len() - t.raw.trim_start_matches([' ', '\t', '\r', '\n']).len();
                    return Err((offset + skip, "text outside an element".to_owned()));
                }
            }
            if !tops.iter().any(|&id| self.doc.is_element(id)) {
                return Err((self.base, "the fragment contains no element".to_owned()));
            }
        }
        Ok(())
    }
}

/// The parts of a start tag.
#[derive(Debug)]
pub(crate) struct StartTag<'a> {
    pub(crate) name: &'a str,
    pub(crate) attributes: Vec<Attribute>,
    pub(crate) tail: &'a str,
    pub(crate) self_closing: bool,
}

/// Splits a start tag (`<name attr="v">` or `<name/>`) into its parts.
///
/// The rules follow `XMLElement::ParseDeep`, `XMLElement::ParseAttributes`
/// and `XMLAttribute::ParseDeep` in `tinyxml2.cpp`:
///
/// - The name follows `<` directly.
/// - White space is allowed around `=`. The value is in `"` or `'`.
/// - White space between two attributes is optional.
/// - Two attributes with the same name are an error.
/// - `/` must be directly before the final `>`.
///
/// The error offset is relative to the start of `raw`.
pub(crate) fn parse_start_tag(raw: &str) -> Result<StartTag<'_>, (usize, String)> {
    let b = raw.as_bytes();
    if b.len() < 2 || b[0] != b'<' || b[b.len() - 1] != b'>' {
        return Err((0, "malformed start tag".to_owned()));
    }
    let self_closing = b.len() >= 3 && b[b.len() - 2] == b'/';
    let end = if self_closing {
        b.len() - 2
    } else {
        b.len() - 1
    };
    let name_l = name_len(&raw[1..end]);
    if name_l == 0 {
        return Err((1, "expected an element name after '<'".to_owned()));
    }
    let name = &raw[1..1 + name_l];
    let mut attributes: Vec<Attribute> = Vec::new();
    let mut i = 1 + name_l;
    loop {
        let ws_start = i;
        while i < end && is_xml_whitespace(b[i]) {
            i += 1;
        }
        if i == end {
            return Ok(StartTag {
                name,
                attributes,
                tail: &raw[ws_start..end],
                self_closing,
            });
        }
        let n = name_len(&raw[i..end]);
        if n == 0 {
            let c = raw[i..].chars().next().unwrap_or('?');
            let msg = if c == '/' {
                format!("'/' must be directly before '>' in the start tag of <{name}>")
            } else {
                format!("unexpected character {c:?} in the start tag of <{name}>")
            };
            return Err((i, msg));
        }
        let attr_name = &raw[i..i + n];
        let name_end = i + n;
        let mut j = name_end;
        while j < end && is_xml_whitespace(b[j]) {
            j += 1;
        }
        if j >= end || b[j] != b'=' {
            return Err((
                j.min(end),
                format!("expected '=' after the attribute name {attr_name:?}"),
            ));
        }
        j += 1;
        while j < end && is_xml_whitespace(b[j]) {
            j += 1;
        }
        let quote = match b.get(j) {
            Some(b'"') if j < end => Quote::Double,
            Some(b'\'') if j < end => Quote::Single,
            _ => {
                return Err((
                    j.min(end),
                    format!("the value of the attribute {attr_name:?} must be in quotes"),
                ));
            }
        };
        let value_start = j + 1;
        let Some(close) = raw[value_start..end].find(quote.as_char()) else {
            return Err((
                j,
                format!("the value of the attribute {attr_name:?} has no closing quote"),
            ));
        };
        let value_end = value_start + close;
        if attributes.iter().any(|a| a.name == attr_name) {
            return Err((i, format!("duplicate attribute {attr_name:?}")));
        }
        attributes.push(Attribute::from_source(
            attr_name,
            &raw[value_start..value_end],
            quote,
            &raw[ws_start..i],
            &raw[name_end..j],
        ));
        i = value_end + 1;
    }
}

/// Finds the first syntax error in a start tag that has no closing `>`
/// before the end of the input. `rest` starts with `<`. Returns `None` if
/// the scan finds no error before the end of the input or before a `>`.
fn diagnose_unclosed_tag(rest: &str) -> Option<(usize, String)> {
    let b = rest.as_bytes();
    if b.first() != Some(&b'<') || matches!(b.get(1), Some(b'/' | b'!' | b'?')) {
        return None;
    }
    let n = name_len(&rest[1..]);
    if n == 0 {
        return Some((1, "expected an element name after '<'".to_owned()));
    }
    let name = &rest[1..1 + n];
    let mut i = 1 + n;
    loop {
        while i < b.len() && is_xml_whitespace(b[i]) {
            i += 1;
        }
        if i >= b.len() || b[i] == b'>' || (b[i] == b'/' && b.get(i + 1) == Some(&b'>')) {
            return None;
        }
        let n = name_len(&rest[i..]);
        if n == 0 {
            let c = rest[i..].chars().next().unwrap_or('?');
            return Some((
                i,
                format!("unexpected character {c:?} in the start tag of <{name}>"),
            ));
        }
        let attr_name = &rest[i..i + n];
        let mut j = i + n;
        while j < b.len() && is_xml_whitespace(b[j]) {
            j += 1;
        }
        if j >= b.len() {
            return None;
        }
        if b[j] != b'=' {
            return Some((
                j,
                format!("expected '=' after the attribute name {attr_name:?}"),
            ));
        }
        j += 1;
        while j < b.len() && is_xml_whitespace(b[j]) {
            j += 1;
        }
        let quote = match b.get(j) {
            None => return None,
            Some(&q @ (b'"' | b'\'')) => q,
            Some(_) => {
                return Some((
                    j,
                    format!("the value of the attribute {attr_name:?} must be in quotes"),
                ));
            }
        };
        let Some(close) = b[j + 1..].iter().position(|&c| c == quote) else {
            return Some((
                j,
                format!("the value of the attribute {attr_name:?} has no closing quote"),
            ));
        };
        i = j + 1 + close + 1;
    }
}

/// Splits an end tag (`</name >`) into the name and the white space before
/// `>`. The error offset is relative to the start of `raw`.
pub(crate) fn parse_end_tag(raw: &str) -> Result<(&str, &str), (usize, String)> {
    let b = raw.as_bytes();
    if b.len() < 3 || !raw.starts_with("</") || b[b.len() - 1] != b'>' {
        return Err((0, "malformed end tag".to_owned()));
    }
    let end = b.len() - 1;
    let n = name_len(&raw[2..end]);
    if n == 0 {
        return Err((2, "expected an element name after '</'".to_owned()));
    }
    let name = &raw[2..2 + n];
    let rest = &raw[2 + n..end];
    if let Some(p) = rest.bytes().position(|c| !is_xml_whitespace(c)) {
        return Err((
            2 + n + p,
            format!("unexpected text in the end tag </{name}>"),
        ));
    }
    Ok((name, rest))
}

/// Finds the indentation unit of a document: the most common difference
/// between the indentation of an element and the indentation of its parent.
/// The default is four spaces.
fn infer_indent_unit(doc: &Document) -> String {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for id in doc.descendants(doc.root()) {
        let Some(parent) = doc.parent(id) else {
            continue;
        };
        if !doc.is_element(id) || !doc.is_element(parent) {
            continue;
        }
        let (Some(child), Some(outer)) = (doc.indentation(id), doc.indentation(parent)) else {
            continue;
        };
        if child.len() > outer.len() && child.starts_with(outer) {
            let unit = &child[outer.len()..];
            match counts.iter_mut().find(|(u, _)| u == unit) {
                Some((_, n)) => *n += 1,
                None => counts.push((unit.to_owned(), 1)),
            }
        }
    }
    counts
        .into_iter()
        .max_by_key(|(_, n)| *n)
        .map(|(u, _)| u)
        .unwrap_or_else(|| DEFAULT_INDENT.to_owned())
}

impl Document {
    /// Returns the indentation of a node: the white space after the last
    /// line end in the white-space text node directly before it. The first
    /// node of the document has the indentation "".
    pub(crate) fn indentation(&self, id: NodeId) -> Option<&str> {
        let Some(prev) = self.previous_sibling(id) else {
            // The first node of the document starts at column 1.
            return (self.parent(id) == Some(self.root())).then_some("");
        };
        match self.node(prev) {
            Some(Node::Text(t)) if t.is_whitespace() => {
                let nl = t.raw.rfind('\n')?;
                Some(&t.raw[nl + 1..])
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_tag_parts() {
        let t = parse_start_tag("<a  x = \"1\"\ty='2' />").unwrap();
        assert_eq!(t.name, "a");
        assert!(t.self_closing);
        assert_eq!(t.tail, " ");
        assert_eq!(t.attributes.len(), 2);
        assert_eq!(t.attributes[0].leading_whitespace(), "  ");
        assert_eq!(t.attributes[0].equals_text(), " = ");
        assert_eq!(t.attributes[0].quote(), Quote::Double);
        assert_eq!(t.attributes[1].leading_whitespace(), "\t");
        assert_eq!(t.attributes[1].quote(), Quote::Single);
        assert_eq!(t.attributes[1].value(), "2");
    }

    #[test]
    fn start_tag_errors() {
        assert!(parse_start_tag("< a>").is_err());
        assert!(parse_start_tag("<a b>").is_err());
        assert!(parse_start_tag("<a b=c>").is_err());
        assert!(parse_start_tag("<a b=\"1\" b='2'>").is_err());
        assert!(parse_start_tag("<a/ >").is_err());
        assert!(parse_start_tag("<a \"x\">").is_err());
        assert!(parse_start_tag("<>").is_err());
        // tinyxml2 accepts attributes without white space between them.
        let t = parse_start_tag("<a b=\"1\"c=\"2\">").unwrap();
        assert_eq!(t.attributes[1].leading_whitespace(), "");
    }

    #[test]
    fn end_tag_parts() {
        assert_eq!(parse_end_tag("</a>").unwrap(), ("a", ""));
        assert_eq!(parse_end_tag("</a \n>").unwrap(), ("a", " \n"));
        assert!(parse_end_tag("</a b>").is_err());
        assert!(parse_end_tag("</>").is_err());
        assert!(parse_end_tag("</ a>").is_err());
    }
}
