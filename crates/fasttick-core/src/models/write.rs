//! Writing models into a `<TreeNodesModel>` element.

use crate::xml::{Change, Document, EditError, NodeId};

use super::NodeModel;
use super::parse::{SECTION_TAG, model_sections};

/// An error from a model write function. The document does not change.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModelWriteError {
    /// The document has no root element.
    #[error("the document has no root element")]
    NoRootElement,
    /// No model element has this ID.
    #[error("no node model with the ID \"{0}\"")]
    NotFound(String),
    /// A model element with this ID exists.
    #[error("a node model with the ID \"{0}\" exists")]
    AlreadyExists(String),
    /// An editing function failed (for example a metadata name is not a
    /// valid XML name).
    #[error(transparent)]
    Edit(#[from] EditError),
}

/// Finds the first model element with this `ID` in the model sections of
/// the document.
pub fn find_model_element(doc: &Document, id: &str) -> Option<NodeId> {
    model_sections(doc).into_iter().find_map(|section| {
        doc.element_children(section)
            .find(|&e| doc.attribute(e, "ID") == Some(id))
    })
}

/// Makes a detached model element in the format of `addNodeModelToXML`
/// (`src/xml_parsing.cpp`):
///
/// - the element name is the kind (`Action`, ..., `SubTree`) and the only
///   attribute is `ID`;
/// - one `input_port`, `output_port` or `inout_port` element for each port,
///   with the attributes `name`, `type` and `default` (if set) and the
///   description as text;
/// - a `<MetadataFields>` element with one `<Metadata name="value"/>` for
///   each metadata pair, if there is metadata.
///
/// The white space between the child elements has line ends only;
/// [`Document::insert_element_pretty`] sets the indentation when the
/// element is inserted.
pub fn build_model_element(doc: &mut Document, model: &NodeModel) -> Result<NodeId, EditError> {
    let element = doc.create_element(model.kind.tag())?;
    doc.set_attribute(element, "ID", &model.id)?;
    let mut children = Vec::new();
    for port in &model.ports {
        let port_element = doc.create_element(port.direction.tag())?;
        doc.set_attribute(port_element, "name", &port.name)?;
        if let Some(type_name) = &port.type_name {
            doc.set_attribute(port_element, "type", type_name)?;
        }
        if let Some(default) = &port.default {
            doc.set_attribute(port_element, "default", default)?;
        }
        if let Some(description) = port.description.as_deref().filter(|d| !d.is_empty()) {
            let text = doc.create_text(description)?;
            doc.append_child(port_element, text)?;
        }
        children.push(port_element);
    }
    if !model.metadata.is_empty() {
        let fields = doc.create_element("MetadataFields")?;
        let mut entries = Vec::new();
        for (name, value) in &model.metadata {
            let entry = doc.create_element("Metadata")?;
            doc.set_attribute(entry, name, value)?;
            entries.push(entry);
        }
        append_lines(doc, fields, &entries)?;
        children.push(fields);
    }
    append_lines(doc, element, &children)?;
    Ok(element)
}

/// Appends `children` to `parent`, with a line end before each child and
/// after the last child.
fn append_lines(doc: &mut Document, parent: NodeId, children: &[NodeId]) -> Result<(), EditError> {
    if children.is_empty() {
        return Ok(());
    }
    for &child in children {
        let ws = doc.create_text("\n")?;
        doc.append_child(parent, ws)?;
        doc.append_child(parent, child)?;
    }
    let ws = doc.create_text("\n")?;
    doc.append_child(parent, ws)?;
    Ok(())
}

/// Adds a model to the first `<TreeNodesModel>` of the document. If the
/// document has no such element, the function adds one as the last child
/// of the root element.
///
/// If the IDs of the existing models are in alphabetical order (as
/// `writeTreeNodesModelXML` writes them), the new model goes to its
/// alphabetical position. Else it goes after the last model.
///
/// Returns the changes that undo the addition (apply them with
/// [`Document::apply_all`]).
pub fn add_model(doc: &mut Document, model: &NodeModel) -> Result<Vec<Change>, ModelWriteError> {
    let root = doc
        .document_element()
        .ok_or(ModelWriteError::NoRootElement)?;
    if find_model_element(doc, &model.id).is_some() {
        return Err(ModelWriteError::AlreadyExists(model.id.clone()));
    }
    let element = build_model_element(doc, model)?;
    match model_sections(doc).first().copied() {
        Some(section) => {
            let ids: Vec<Option<&str>> = doc
                .element_children(section)
                .map(|e| doc.attribute(e, "ID"))
                .collect();
            let sorted = ids.windows(2).all(|w| w[0] <= w[1]);
            let index = if sorted {
                ids.iter()
                    .position(|id| id.is_some_and(|id| id > model.id.as_str()))
                    .unwrap_or(ids.len())
            } else {
                ids.len()
            };
            Ok(doc.insert_element_pretty(section, index, element)?)
        }
        None => {
            let section = doc.create_element(SECTION_TAG)?;
            append_lines(doc, section, &[element])?;
            let index = doc.element_children(root).count();
            Ok(doc.insert_element_pretty(root, index, section)?)
        }
    }
}

/// Replaces the model element with the ID `id` by a new element for
/// `model`. The ID and the kind can change. The new element takes the place
/// of the old one; the old one is detached.
///
/// Returns the changes that undo the replacement.
pub fn replace_model(
    doc: &mut Document,
    id: &str,
    model: &NodeModel,
) -> Result<Vec<Change>, ModelWriteError> {
    let old =
        find_model_element(doc, id).ok_or_else(|| ModelWriteError::NotFound(id.to_owned()))?;
    if model.id != id && find_model_element(doc, &model.id).is_some() {
        return Err(ModelWriteError::AlreadyExists(model.id.clone()));
    }
    let section = doc
        .parent(old)
        .ok_or_else(|| ModelWriteError::NotFound(id.to_owned()))?;
    let index = doc
        .element_children(section)
        .position(|e| e == old)
        .ok_or_else(|| ModelWriteError::NotFound(id.to_owned()))?;
    let element = build_model_element(doc, model)?;
    let undo_insert = doc.insert_element_pretty(section, index, element)?;
    let undo_remove = match doc.remove_element_pretty(old) {
        Ok(undo) => undo,
        Err(e) => {
            // Undo the insertion; its inverse is valid.
            let _ = doc.apply_all(undo_insert);
            return Err(e.into());
        }
    };
    // Undo in reverse order: first put the old element back, then remove
    // the new one.
    let mut undo = undo_remove;
    undo.extend(undo_insert);
    Ok(undo)
}

/// Removes the model element with this ID, with its white space.
///
/// Returns the changes that undo the removal.
pub fn remove_model(doc: &mut Document, id: &str) -> Result<Vec<Change>, ModelWriteError> {
    let element =
        find_model_element(doc, id).ok_or_else(|| ModelWriteError::NotFound(id.to_owned()))?;
    Ok(doc.remove_element_pretty(element)?)
}

/// Writes a complete models file, like `writeTreeNodesModelXML`
/// (`src/xml_parsing.cpp`): `<root BTCPP_format="4">` with one
/// `<TreeNodesModel>` that holds the models sorted by ID (BT.CPP uses a
/// `std::map`). The ports keep their order. Indentation is four spaces;
/// the text ends with a line end.
pub fn write_models_document(models: &[NodeModel]) -> Result<String, ModelWriteError> {
    let mut doc = Document::parse("<root BTCPP_format=\"4\">\n    <TreeNodesModel/>\n</root>\n")
        .map_err(|e| ModelWriteError::Edit(EditError::InvalidContent(e.to_string())))?;
    let mut sorted: Vec<&NodeModel> = models.iter().collect();
    sorted.sort_by(|a, b| a.id.cmp(&b.id));
    sorted.dedup_by(|a, b| a.id == b.id);
    let section = model_sections(&doc)
        .first()
        .copied()
        .ok_or(ModelWriteError::NoRootElement)?;
    for (index, model) in sorted.into_iter().enumerate() {
        let element = build_model_element(&mut doc, model)?;
        doc.insert_element_pretty(section, index, element)?;
    }
    Ok(doc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        ModelSource, NodeKind, PortDirection, PortModel, parse_models_document, parse_models_text,
    };

    fn sample() -> NodeModel {
        let mut model = NodeModel::new("Move", NodeKind::Action);
        model.ports.push(PortModel {
            name: "goal".into(),
            direction: PortDirection::Input,
            type_name: Some("Pose2D".into()),
            default: None,
            description: Some("Where to go & why".into()),
        });
        model.ports.push(PortModel {
            name: "result".into(),
            direction: PortDirection::Output,
            type_name: Some("std::vector<int>".into()),
            default: Some("".into()),
            description: None,
        });
        model
            .metadata
            .push(("description".into(), "Moves \"the\" robot".into()));
        model
    }

    #[test]
    fn build_format() {
        let mut doc = Document::parse("<root>\n    <TreeNodesModel>\n        <Action ID=\"A\"/>\n    </TreeNodesModel>\n</root>\n").unwrap();
        add_model(&mut doc, &sample()).unwrap();
        assert_eq!(
            doc.to_string(),
            "<root>\n    <TreeNodesModel>\n        <Action ID=\"A\"/>\n        \
             <Action ID=\"Move\">\n            \
             <input_port name=\"goal\" type=\"Pose2D\">Where to go &amp; why</input_port>\n            \
             <output_port name=\"result\" type=\"std::vector&lt;int&gt;\" default=\"\"/>\n            \
             <MetadataFields>\n                \
             <Metadata description=\"Moves &quot;the&quot; robot\"/>\n            \
             </MetadataFields>\n        \
             </Action>\n    </TreeNodesModel>\n</root>\n"
        );
    }

    #[test]
    fn add_replace_remove_round_trip_and_undo() {
        let text = "<?xml version=\"1.0\"?>\r\n<root BTCPP_format=\"4\">\r\n\t<BehaviorTree ID=\"T\">\r\n\t\t<AlwaysSuccess/>\r\n\t</BehaviorTree>\r\n</root>\r\n";
        let mut doc = Document::parse(text).unwrap();
        // No section yet: one is made.
        let undo_add = add_model(&mut doc, &sample()).unwrap();
        let written = doc.to_string();
        assert!(
            written.contains(
                "\r\n\t<TreeNodesModel>\r\n\t\t<Action ID=\"Move\">\r\n\t\t\t<input_port"
            )
        );
        let parsed = parse_models_document(&doc, ModelSource::Unattached);
        assert!(parsed.issues.is_empty());
        assert_eq!(parsed.models.len(), 1);
        assert_eq!(parsed.models[0].model, sample());
        assert_eq!(
            add_model(&mut doc, &sample()),
            Err(ModelWriteError::AlreadyExists("Move".into()))
        );

        // Replace with a changed model (other ID and kind).
        let mut changed = sample();
        changed.id = "Walk".into();
        changed.kind = NodeKind::Condition;
        changed.ports.pop();
        changed.metadata.clear();
        let undo_replace = replace_model(&mut doc, "Move", &changed).unwrap();
        let parsed = parse_models_document(&doc, ModelSource::Unattached);
        assert_eq!(parsed.models.len(), 1);
        assert_eq!(parsed.models[0].model, changed);
        assert_eq!(
            replace_model(&mut doc, "Move", &changed),
            Err(ModelWriteError::NotFound("Move".into()))
        );

        // Remove.
        let undo_remove = remove_model(&mut doc, "Walk").unwrap();
        assert!(
            parse_models_document(&doc, ModelSource::Unattached)
                .models
                .is_empty()
        );
        assert_eq!(
            remove_model(&mut doc, "Walk"),
            Err(ModelWriteError::NotFound("Walk".into()))
        );

        // Undo everything, in reverse order.
        doc.apply_all(undo_remove).unwrap();
        doc.apply_all(undo_replace).unwrap();
        assert_eq!(doc.to_string(), written);
        doc.apply_all(undo_add).unwrap();
        assert_eq!(doc.to_string(), text);
    }

    #[test]
    fn sorted_insert_and_unsorted_append() {
        let mut doc = Document::parse(
            "<root>\n  <TreeNodesModel>\n    <Action ID=\"A\"/>\n    <Action ID=\"C\"/>\n  </TreeNodesModel>\n</root>",
        )
        .unwrap();
        add_model(&mut doc, &NodeModel::new("B", NodeKind::Action)).unwrap();
        assert_eq!(
            doc.to_string(),
            "<root>\n  <TreeNodesModel>\n    <Action ID=\"A\"/>\n    <Action ID=\"B\"/>\n    <Action ID=\"C\"/>\n  </TreeNodesModel>\n</root>"
        );
        let mut doc = Document::parse(
            "<root><TreeNodesModel><Action ID=\"C\"/><Action ID=\"A\"/></TreeNodesModel></root>",
        )
        .unwrap();
        add_model(&mut doc, &NodeModel::new("B", NodeKind::Action)).unwrap();
        assert_eq!(
            doc.to_string(),
            "<root><TreeNodesModel><Action ID=\"C\"/><Action ID=\"A\"/><Action ID=\"B\"/></TreeNodesModel></root>"
        );
    }

    #[test]
    fn errors_leave_document_unchanged() {
        let mut doc = Document::parse("<!-- x --><root/>").unwrap();
        let mut bad = NodeModel::new("X", NodeKind::Action);
        bad.metadata.push(("not a name".into(), "v".into()));
        let before = doc.to_string();
        assert!(matches!(
            add_model(&mut doc, &bad),
            Err(ModelWriteError::Edit(_))
        ));
        assert_eq!(doc.to_string(), before);

        let mut empty = Document::parse("<!-- only -->").unwrap();
        assert_eq!(
            add_model(&mut empty, &NodeModel::new("X", NodeKind::Action)),
            Err(ModelWriteError::NoRootElement)
        );
    }

    #[test]
    fn whole_document() {
        let mut b = NodeModel::new("B", NodeKind::Decorator);
        b.ports.push(PortModel::new("p", PortDirection::InOut));
        let models = vec![b, NodeModel::new("A", NodeKind::Action), sample()];
        let text = write_models_document(&models).unwrap();
        assert!(text.starts_with("<root BTCPP_format=\"4\">\n    <TreeNodesModel>\n        <Action ID=\"A\"/>\n        <Decorator ID=\"B\">\n            <inout_port name=\"p\"/>\n        </Decorator>\n"));
        assert!(text.ends_with("    </TreeNodesModel>\n</root>\n"));
        let parsed = parse_models_text(&text, ModelSource::Unattached).unwrap();
        let ids: Vec<&str> = parsed.models.iter().map(|m| m.model.id.as_str()).collect();
        assert_eq!(ids, vec!["A", "B", "Move"]);
        assert_eq!(parsed.models[2].model, sample());
        // No models: an empty section.
        let text = write_models_document(&[]).unwrap();
        assert_eq!(
            text,
            "<root BTCPP_format=\"4\">\n    <TreeNodesModel/>\n</root>\n"
        );
    }
}
