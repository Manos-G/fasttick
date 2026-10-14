//! Lossless XML document model.
//!
//! [`Document`] keeps all parts of an XML file: the attribute order, the
//! quote characters, the original escapes, the comments, the white space,
//! the line ends and the byte order mark. If nothing changed, the writer
//! gives the same bytes as the input. After a change, the writer makes new
//! text only for the changed elements.
//!
//! The parser uses the event reader of `quick-xml` to find the markup. It
//! checks each tag with the rules of tinyxml2, the XML reader that
//! BehaviorTree.CPP uses (`3rdparty/tinyxml2` in the BT.CPP source). The
//! decoded values of attributes and text are the values that BT.CPP sees.
//!
//! # Decoded values
//!
//! [`Attribute::value`] and [`Text::decoded`] give the values that BT.CPP
//! sees, with the unusual cases of tinyxml2:
//!
//! - Line ends become LF (also LF CR); tabs and line ends are kept.
//! - Only the five predefined entities and `&#...;` references are
//!   decoded. Only a lowercase `x` starts a hexadecimal reference. A
//!   reference to U+0000 ends the value.
//! - An `&` that does not start a reference stays, but only while nothing
//!   before it in the value became shorter. tinyxml2 decodes in place and
//!   does not copy such an `&`; after a shorter reference, the byte at the
//!   write position is a stale source byte. For example, BT.CPP reads
//!   `a &lt; b && c` as `a < b  b c`. Write `&amp;` to be safe.
//!
//! [`Attribute::raw_value`] and [`Text::raw`] give the source text.
//!
//! # Example
//!
//! ```
//! use fasttick_core::xml::Document;
//!
//! let text = "<root BTCPP_format=\"4\">\n    <BehaviorTree ID='Main'/>\n</root>\n";
//! let mut doc = Document::parse(text).unwrap();
//! assert_eq!(doc.to_string(), text);
//!
//! let root = doc.document_element().unwrap();
//! let tree = doc.element_children(root).next().unwrap();
//! assert_eq!(doc.attribute(tree, "ID"), Some("Main"));
//!
//! let change = doc.set_attribute(tree, "ID", "Other").unwrap();
//! assert_eq!(
//!     doc.to_string(),
//!     "<root BTCPP_format=\"4\">\n    <BehaviorTree ID='Other'/>\n</root>\n"
//! );
//!
//! // Undo.
//! doc.apply(change.inverse()).unwrap();
//! assert_eq!(doc.to_string(), text);
//! ```
//!
//! Paste a node with the indentation of its new siblings, then undo:
//!
//! ```
//! use fasttick_core::xml::Document;
//!
//! let text = "<root>\n    <Sequence>\n        <A/>\n    </Sequence>\n</root>\n";
//! let mut doc = Document::parse(text).unwrap();
//! let sequence = doc.descendants(doc.root()).find(|&n| doc.name(n) == Some("Sequence")).unwrap();
//!
//! let pasted = doc.parse_fragment("<B port=\"{value}\"/>").unwrap();
//! let undo = doc.insert_element_pretty(sequence, 1, pasted[0]).unwrap();
//! assert_eq!(
//!     doc.to_string(),
//!     "<root>\n    <Sequence>\n        <A/>\n        <B port=\"{value}\"/>\n    </Sequence>\n</root>\n"
//! );
//!
//! let redo = doc.apply_all(undo).unwrap();
//! assert_eq!(doc.to_string(), text);
//! doc.apply_all(redo).unwrap();
//! ```

mod document;
mod edit;
mod error;
mod escape;
mod node;
mod parse;
mod pretty;

pub use document::{Descendants, Document, LineEnding};
pub use edit::{AttributeChange, Change, Position};
pub use error::{EditError, XmlError};
pub use escape::is_valid_name;
pub use node::{
    Attribute, CData, Comment, Declaration, DocType, Element, Node, NodeId, NodeKind,
    ProcessingInstruction, Quote, Text,
};
pub use parse::MAX_ELEMENT_DEPTH;
