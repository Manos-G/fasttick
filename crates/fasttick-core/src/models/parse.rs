//! Reading models from `<TreeNodesModel>` elements.

use std::collections::HashSet;
use std::fmt;

use crate::xml::{Document, NodeId, XmlError};

use super::{ModelSource, NodeKind, NodeModel, PortDirection, PortModel};

/// The name of a model section element.
pub(crate) const SECTION_TAG: &str = "TreeNodesModel";

/// One model and its place in the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedModel {
    /// The model element (for example `<Action ID="X">`).
    pub element: NodeId,
    /// The `<TreeNodesModel>` element that contains it.
    pub section: NodeId,
    /// The model.
    pub model: NodeModel,
}

/// A problem in a model section. The parser skips the bad part and
/// continues.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelIssue {
    /// The element with the problem.
    pub element: NodeId,
    /// The problem.
    pub kind: ModelIssueKind,
}

/// The kind of a [`ModelIssue`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelIssueKind {
    /// A child of `<TreeNodesModel>` is not `Action`, `Condition`,
    /// `Control`, `Decorator` or `SubTree`. The parser skips it.
    UnknownModelTag(String),
    /// A model element has no `ID` attribute. The parser skips it.
    /// (`loadSubtreeModel` in BT.CPP throws for a `<SubTree>` model
    /// without `ID`.)
    MissingId,
    /// A port element has no `name` attribute. The parser skips the port.
    /// (`parseSubtreeModelPorts` in BT.CPP throws.)
    MissingPortName,
    /// Two ports of one model have the same name. The parser keeps the
    /// first.
    DuplicatePort(String),
    /// Two models in the same document have the same ID. Look-ups use the
    /// first.
    DuplicateModel(String),
}

impl fmt::Display for ModelIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.kind, f)
    }
}

impl fmt::Display for ModelIssueKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModelIssueKind::UnknownModelTag(tag) => write!(
                f,
                "<{tag}> is not a node model; use Action, Condition, Control, Decorator or SubTree"
            ),
            ModelIssueKind::MissingId => f.write_str("the node model has no ID attribute"),
            ModelIssueKind::MissingPortName => f.write_str("the port has no name attribute"),
            ModelIssueKind::DuplicatePort(name) => {
                write!(f, "the port \"{name}\" is defined more than once")
            }
            ModelIssueKind::DuplicateModel(id) => {
                write!(f, "the node model \"{id}\" is defined more than once")
            }
        }
    }
}

/// The models and the problems of a document or a section.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedModels {
    /// The models, in document order.
    pub models: Vec<ParsedModel>,
    /// The problems, in document order.
    pub issues: Vec<ModelIssue>,
}

impl ParsedModels {
    /// The first model with this ID.
    pub fn get(&self, id: &str) -> Option<&ParsedModel> {
        self.models.iter().find(|m| m.model.id == id)
    }

    /// Only the models.
    pub fn into_models(self) -> Vec<NodeModel> {
        self.models.into_iter().map(|m| m.model).collect()
    }
}

/// True if the element is a `<TreeNodesModel>`.
pub fn is_model_section(doc: &Document, id: NodeId) -> bool {
    doc.name(id) == Some(SECTION_TAG)
}

/// The `<TreeNodesModel>` elements of a document: the element children of
/// the document element with this name. If the document element itself is
/// a `<TreeNodesModel>`, the result is only that element.
pub fn model_sections(doc: &Document) -> Vec<NodeId> {
    let Some(root) = doc.document_element() else {
        return Vec::new();
    };
    if is_model_section(doc, root) {
        return vec![root];
    }
    doc.element_children(root)
        .filter(|&c| is_model_section(doc, c))
        .collect()
}

/// Reads one model element (for example `<Action ID="X">`).
///
/// Port elements are `input_port`, `output_port` and `inout_port`. The
/// description is the text of the port element (the format of
/// `writeTreeNodesModelXML`); if there is no text, it is the `description`
/// attribute (the format that `parseSubtreeModelPorts` reads). The
/// metadata are the attributes of the `<Metadata>` elements in
/// `<MetadataFields>`, in order. Other child elements are ignored.
///
/// Problems with single ports go into `issues`; the function then skips the
/// port.
pub fn parse_model_element(
    doc: &Document,
    element: NodeId,
    source: ModelSource,
    issues: &mut Vec<ModelIssue>,
) -> Option<NodeModel> {
    let tag = doc.name(element)?;
    let Some(kind) = NodeKind::from_tag(tag) else {
        issues.push(ModelIssue {
            element,
            kind: ModelIssueKind::UnknownModelTag(tag.to_owned()),
        });
        return None;
    };
    let Some(id) = doc.attribute(element, "ID") else {
        issues.push(ModelIssue {
            element,
            kind: ModelIssueKind::MissingId,
        });
        return None;
    };
    let mut model = NodeModel::new(id, kind);
    model.source = source;
    let mut seen = HashSet::new();
    for child in doc.element_children(element) {
        let child_tag = doc.name(child).unwrap_or_default();
        if let Some(direction) = PortDirection::from_tag(child_tag) {
            let Some(name) = doc.attribute(child, "name") else {
                issues.push(ModelIssue {
                    element: child,
                    kind: ModelIssueKind::MissingPortName,
                });
                continue;
            };
            if !seen.insert(name) {
                issues.push(ModelIssue {
                    element: child,
                    kind: ModelIssueKind::DuplicatePort(name.to_owned()),
                });
                continue;
            }
            let description = doc
                .get_text(child)
                .or_else(|| doc.attribute(child, "description").map(str::to_owned))
                .filter(|d| !d.is_empty());
            model.ports.push(PortModel {
                name: name.to_owned(),
                direction,
                type_name: doc.attribute(child, "type").map(str::to_owned),
                default: doc.attribute(child, "default").map(str::to_owned),
                description,
            });
        } else if child_tag == "MetadataFields" {
            for field in doc.element_children(child) {
                if doc.name(field) != Some("Metadata") {
                    continue;
                }
                for attribute in doc.attributes(field) {
                    model
                        .metadata
                        .push((attribute.name().to_owned(), attribute.value().to_owned()));
                }
            }
        }
    }
    Some(model)
}

/// Reads all models of one `<TreeNodesModel>` element and adds them to
/// `out`. Duplicate IDs (also with the models already in `out`) become
/// issues; the models stay in the list.
pub fn parse_model_section(
    doc: &Document,
    section: NodeId,
    source: ModelSource,
    out: &mut ParsedModels,
) {
    for element in doc.element_children(section) {
        let Some(model) = parse_model_element(doc, element, source, &mut out.issues) else {
            continue;
        };
        if out.models.iter().any(|m| m.model.id == model.id) {
            out.issues.push(ModelIssue {
                element,
                kind: ModelIssueKind::DuplicateModel(model.id.clone()),
            });
        }
        out.models.push(ParsedModel {
            element,
            section,
            model,
        });
    }
}

/// Reads the models of all sections of a document (see
/// [`model_sections`]).
pub fn parse_models_document(doc: &Document, source: ModelSource) -> ParsedModels {
    let mut out = ParsedModels::default();
    for section in model_sections(doc) {
        parse_model_section(doc, section, source, &mut out);
    }
    out
}

/// Parses a models XML text (for example the output of
/// `writeTreeNodesModelXML`) and reads its models.
pub fn parse_models_text(text: &str, source: ModelSource) -> Result<ParsedModels, XmlError> {
    let doc = Document::parse(text)?;
    Ok(parse_models_document(&doc, source))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = r#"<root BTCPP_format="4">
    <TreeNodesModel>
        <Action ID="A">
            <input_port name="x" type="int" default="1">X value</input_port>
            <output_port name="y"/>
            <inout_port name="z" type="std::string" description="from attribute"/>
            <input_port name="w" description="ignored">text wins</input_port>
            <MetadataFields>
                <Metadata description="d"/>
                <Metadata a="1" b="2"/>
                <Other c="3"/>
            </MetadataFields>
            <description>ignored element</description>
        </Action>
        <SubTree ID="S">
            <input_port name="goal"/>
        </SubTree>
        <Unknown ID="U"/>
        <Action/>
        <Condition ID="C">
            <input_port/>
            <input_port name="p"/>
            <output_port name="p"/>
        </Condition>
        <Action ID="A"/>
    </TreeNodesModel>
</root>"#;

    #[test]
    fn parses_ports_metadata_and_issues() {
        let parsed = parse_models_text(TEXT, ModelSource::Unattached).unwrap();
        let ids: Vec<&str> = parsed.models.iter().map(|m| m.model.id.as_str()).collect();
        assert_eq!(ids, vec!["A", "S", "C", "A"]);
        let a = &parsed.get("A").unwrap().model;
        assert_eq!(a.kind, NodeKind::Action);
        assert_eq!(a.ports.len(), 4);
        assert_eq!(
            a.ports[0],
            PortModel {
                name: "x".into(),
                direction: PortDirection::Input,
                type_name: Some("int".into()),
                default: Some("1".into()),
                description: Some("X value".into()),
            }
        );
        assert_eq!(a.ports[1].direction, PortDirection::Output);
        assert_eq!(a.ports[1].description, None);
        assert_eq!(a.ports[2].description.as_deref(), Some("from attribute"));
        assert_eq!(a.ports[3].description.as_deref(), Some("text wins"));
        assert_eq!(
            a.metadata,
            vec![
                ("description".to_owned(), "d".to_owned()),
                ("a".to_owned(), "1".to_owned()),
                ("b".to_owned(), "2".to_owned()),
            ]
        );
        let s = &parsed.get("S").unwrap().model;
        assert_eq!(s.kind, NodeKind::SubTree);
        let c = &parsed.get("C").unwrap().model;
        assert_eq!(c.ports.len(), 1);

        let kinds: Vec<&ModelIssueKind> = parsed.issues.iter().map(|i| &i.kind).collect();
        assert_eq!(
            kinds,
            vec![
                &ModelIssueKind::UnknownModelTag("Unknown".into()),
                &ModelIssueKind::MissingId,
                &ModelIssueKind::MissingPortName,
                &ModelIssueKind::DuplicatePort("p".into()),
                &ModelIssueKind::DuplicateModel("A".into()),
            ]
        );
        for issue in &parsed.issues {
            assert!(!issue.to_string().is_empty());
        }
    }

    #[test]
    fn sections() {
        let doc = Document::parse(
            "<root><TreeNodesModel/><BehaviorTree/><TreeNodesModel><Action ID=\"B\"/></TreeNodesModel></root>",
        )
        .unwrap();
        assert_eq!(model_sections(&doc).len(), 2);
        let parsed = parse_models_document(&doc, ModelSource::Builtin);
        assert_eq!(parsed.models.len(), 1);
        assert_eq!(parsed.models[0].model.source, ModelSource::Builtin);
        assert_eq!(parsed.models[0].section, model_sections(&doc)[1]);

        // A document whose root is the section.
        let doc =
            Document::parse("<TreeNodesModel><Decorator ID=\"D\"/></TreeNodesModel>").unwrap();
        let parsed = parse_models_document(&doc, ModelSource::Unattached);
        assert_eq!(parsed.models[0].model.kind, NodeKind::Decorator);

        // No root element.
        let doc = Document::parse("<!-- only a comment -->").unwrap();
        assert!(model_sections(&doc).is_empty());
        assert!(parse_models_text("<root", ModelSource::Unattached).is_err());
    }

    #[test]
    fn description_with_escapes_and_cdata() {
        let parsed = parse_models_text(
            "<root><TreeNodesModel><Action ID=\"A\">\
             <input_port name=\"a\">x &amp; y</input_port>\
             <input_port name=\"b\"><![CDATA[<raw>]]></input_port>\
             <input_port name=\"c\">   </input_port>\
             </Action></TreeNodesModel></root>",
            ModelSource::Unattached,
        )
        .unwrap();
        let a = &parsed.models[0].model;
        assert_eq!(a.ports[0].description.as_deref(), Some("x & y"));
        assert_eq!(a.ports[1].description.as_deref(), Some("<raw>"));
        assert_eq!(a.ports[2].description, None);
    }
}
