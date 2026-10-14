//! A read view of one tree node element.

use crate::models::NodeKind;
use crate::xml::{Document, Element, NodeId};

use super::attr::{AttributeClass, PostCondition, PreCondition};
use super::names::parse_bool;
use super::port_value::PortValue;

/// One attribute of a node element, with its class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeAttribute<'a> {
    /// The index of the attribute in the element.
    pub index: usize,
    /// The attribute name.
    pub name: &'a str,
    /// The decoded value, as BT.CPP sees it.
    pub value: &'a str,
    /// The class of the attribute.
    pub class: AttributeClass,
}

/// A read view of a tree node element (an element below a
/// `<BehaviorTree>`, or a detached node).
///
/// The rules follow `createNodeFromXML` (`src/xml_parsing.cpp`):
///
/// - Compact form, `<Sequence>`: the type ID is the element name. An `ID`
///   attribute is an error in BT.CPP ("Attribute [ID] is not allowed").
/// - Explicit form, `<Action ID="X">`, `<Condition ID="X">`,
///   `<Control ID="X">`, `<Decorator ID="X">`: the type ID is the value of
///   `ID`. BT.CPP does not compare the tag with the registered type of `X`;
///   the tag is only a label. `ID` is mandatory.
/// - `<SubTree ID="X">` uses the same code path as the explicit form: the
///   type ID is `X` (the tree ID). The node is created with the registration
///   ID `SubTree` (see [`NodeView::registration_id`]).
/// - The instance name is the `name` attribute (also when it is empty), or
///   the type ID when there is no `name` attribute.
#[derive(Debug, Clone, Copy)]
pub struct NodeView<'a> {
    doc: &'a Document,
    id: NodeId,
    element: &'a Element,
}

impl<'a> NodeView<'a> {
    /// Makes a view of an element. Returns `None` if `id` is not an element.
    pub fn new(doc: &'a Document, id: NodeId) -> Option<NodeView<'a>> {
        doc.element(id).map(|element| NodeView { doc, id, element })
    }

    /// The document.
    pub fn doc(&self) -> &'a Document {
        self.doc
    }

    /// The element.
    pub fn id(&self) -> NodeId {
        self.id
    }

    /// The element data.
    pub fn element(&self) -> &'a Element {
        self.element
    }

    /// The element name.
    pub fn tag(&self) -> &'a str {
        self.element.name()
    }

    /// The category of the tag, if the tag is `Action`, `Condition`,
    /// `Control`, `Decorator` or `SubTree` (`convertFromString<NodeType>` in
    /// `src/basic_types.cpp`).
    pub fn category_tag(&self) -> Option<NodeKind> {
        NodeKind::from_tag(self.tag())
    }

    /// True for the explicit form `<Action|Condition|Control|Decorator
    /// ID="X">`. `<SubTree>` is not included.
    pub fn is_explicit_form(&self) -> bool {
        matches!(
            self.category_tag(),
            Some(NodeKind::Action | NodeKind::Condition | NodeKind::Control | NodeKind::Decorator)
        )
    }

    /// True for a `<SubTree>` element.
    pub fn is_subtree(&self) -> bool {
        self.tag() == "SubTree"
    }

    /// The type ID (`type_ID` in `createNodeFromXML`): the element name for
    /// the compact form, the `ID` attribute for the explicit form and for
    /// `<SubTree>`. `None` if the explicit form has no `ID`.
    pub fn type_id(&self) -> Option<&'a str> {
        if self.category_tag().is_some() {
            self.element.attribute("ID")
        } else {
            Some(self.tag())
        }
    }

    /// The registration ID of the node that BT.CPP creates: `SubTree` for a
    /// `<SubTree>`, else the type ID. The runtime XML (`WriteTreeToXML`)
    /// uses it as the element name.
    pub fn registration_id(&self) -> Option<&'a str> {
        if self.is_subtree() {
            Some("SubTree")
        } else {
            self.type_id()
        }
    }

    /// The target tree ID of a `<SubTree>` element.
    pub fn subtree_id(&self) -> Option<&'a str> {
        if self.is_subtree() {
            self.element.attribute("ID")
        } else {
            None
        }
    }

    /// The value of the `name` attribute.
    pub fn name_attribute(&self) -> Option<&'a str> {
        self.element.attribute("name")
    }

    /// The instance name: the `name` attribute, or the type ID.
    pub fn instance_name(&self) -> Option<&'a str> {
        self.name_attribute().or_else(|| self.type_id())
    }

    /// The decoded value of an attribute.
    pub fn attribute(&self, name: &str) -> Option<&'a str> {
        self.element.attribute(name)
    }

    /// All attributes, in document order, with their class.
    pub fn attributes(&self) -> impl Iterator<Item = NodeAttribute<'a>> + 'a {
        self.element
            .attributes()
            .iter()
            .enumerate()
            .map(|(index, a)| NodeAttribute {
                index,
                name: a.name(),
                value: a.value(),
                class: AttributeClass::of(a.name()),
            })
    }

    /// The port attributes (or SubTree remappings), in document order.
    pub fn ports(&self) -> impl Iterator<Item = (&'a str, &'a str)> + 'a {
        self.attributes()
            .filter(|a| a.class.is_port())
            .map(|a| (a.name, a.value))
    }

    /// The parsed value of a port attribute. For a `<SubTree>`, the value is
    /// parsed with [`PortValue::parse_remapping`], else with
    /// [`PortValue::parse`]. `None` if the attribute is missing or is not a
    /// port.
    pub fn port_value(&self, name: &str) -> Option<PortValue<'a>> {
        if !AttributeClass::of(name).is_port() {
            return None;
        }
        let value = self.attribute(name)?;
        Some(if self.is_subtree() {
            PortValue::parse_remapping(value)
        } else {
            PortValue::parse(value)
        })
    }

    /// The script of one pre-condition.
    pub fn pre_condition(&self, condition: PreCondition) -> Option<&'a str> {
        self.attribute(condition.attribute_name())
    }

    /// The script of one post-condition.
    pub fn post_condition(&self, condition: PostCondition) -> Option<&'a str> {
        self.attribute(condition.attribute_name())
    }

    /// The pre-conditions, in the order in which BT.CPP evaluates them
    /// ([`PreCondition::ALL`]), not in the attribute order.
    pub fn pre_conditions(&self) -> impl Iterator<Item = (PreCondition, &'a str)> + 'a {
        let view = *self;
        PreCondition::ALL
            .into_iter()
            .filter_map(move |c| view.pre_condition(c).map(|s| (c, s)))
    }

    /// The post-conditions, in the order of [`PostCondition::ALL`].
    pub fn post_conditions(&self) -> impl Iterator<Item = (PostCondition, &'a str)> + 'a {
        let view = *self;
        PostCondition::ALL
            .into_iter()
            .filter_map(move |c| view.post_condition(c).map(|s| (c, s)))
    }

    /// The value of `_autoremap`.
    pub fn autoremap_attribute(&self) -> Option<&'a str> {
        self.attribute("_autoremap")
    }

    /// The value of `_autoremap` as a boolean: `Some(Ok(b))` for a valid
    /// value, `Some(Err(text))` for a value that `convertFromString<bool>`
    /// rejects, `None` if the attribute is missing (BT.CPP then uses
    /// `false`).
    ///
    /// Like `recursivelyCreateSubtree`, the value `{=}` becomes
    /// `{_autoremap}` first; that is not a valid boolean.
    pub fn autoremap(&self) -> Option<Result<bool, &'a str>> {
        let text = self.autoremap_attribute()?;
        Some(parse_bool(text).ok_or(text))
    }

    /// The value of `_uid` (only in the runtime XML).
    pub fn uid_attribute(&self) -> Option<&'a str> {
        self.attribute("_uid")
    }

    /// The child elements, as views.
    pub fn children(&self) -> impl Iterator<Item = NodeView<'a>> + 'a {
        let doc = self.doc;
        doc.element_children(self.id)
            .filter_map(move |c| NodeView::new(doc, c))
    }

    /// The number of child elements.
    pub fn child_count(&self) -> usize {
        self.doc.element_children(self.id).count()
    }

    /// The parent element.
    pub fn parent(&self) -> Option<NodeView<'a>> {
        self.doc
            .parent(self.id)
            .and_then(|p| NodeView::new(self.doc, p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first(doc: &Document, name: &str) -> NodeId {
        doc.descendants(doc.root())
            .find(|&n| doc.name(n) == Some(name))
            .unwrap()
    }

    #[test]
    fn compact_and_explicit_forms() {
        let doc = Document::parse(
            r#"<root><BehaviorTree ID="T"><Sequence name="s">
                <Action ID="Talk" name="t" msg="{m}"/>
                <SaySomething msg="hi"/>
                <SubTree ID="Sub" name="sub" goal="{=}" text="=" _autoremap="true"/>
                <Control ID="Fallback"><A/></Control>
                <Action/>
            </Sequence></BehaviorTree></root>"#,
        )
        .unwrap();
        let seq = NodeView::new(&doc, first(&doc, "Sequence")).unwrap();
        assert_eq!(seq.type_id(), Some("Sequence"));
        assert_eq!(seq.instance_name(), Some("s"));
        assert_eq!(seq.category_tag(), None);
        assert!(!seq.is_explicit_form());
        let kids: Vec<NodeView> = seq.children().collect();
        assert_eq!(kids.len(), 5);
        assert_eq!(seq.child_count(), 5);

        let talk = kids[0];
        assert!(talk.is_explicit_form());
        assert_eq!(talk.category_tag(), Some(NodeKind::Action));
        assert_eq!(talk.type_id(), Some("Talk"));
        assert_eq!(talk.registration_id(), Some("Talk"));
        assert_eq!(talk.instance_name(), Some("t"));
        assert_eq!(talk.ports().collect::<Vec<_>>(), vec![("msg", "{m}")]);
        assert_eq!(talk.parent().unwrap().id(), seq.id());

        let say = kids[1];
        assert_eq!(say.type_id(), Some("SaySomething"));
        assert_eq!(say.instance_name(), Some("SaySomething"));
        assert_eq!(say.port_value("msg"), Some(PortValue::Literal("hi")));
        assert_eq!(say.port_value("missing"), None);

        let sub = kids[2];
        assert!(sub.is_subtree());
        assert!(!sub.is_explicit_form());
        assert_eq!(sub.category_tag(), Some(NodeKind::SubTree));
        assert_eq!(sub.type_id(), Some("Sub"));
        assert_eq!(sub.subtree_id(), Some("Sub"));
        assert_eq!(sub.registration_id(), Some("SubTree"));
        assert_eq!(sub.autoremap(), Some(Ok(true)));
        assert_eq!(sub.port_value("goal"), Some(PortValue::SameName));
        // A plain "=" is a literal in a SubTree remapping.
        assert_eq!(sub.port_value("text"), Some(PortValue::Literal("=")));
        assert_eq!(sub.port_value("_autoremap"), None);

        let control = kids[3];
        assert_eq!(control.type_id(), Some("Fallback"));
        assert_eq!(control.child_count(), 1);

        let no_id = kids[4];
        assert_eq!(no_id.type_id(), None);
        assert_eq!(no_id.instance_name(), None);
        assert_eq!(say.subtree_id(), None);
    }

    #[test]
    fn conditions_in_btcpp_order() {
        let doc = Document::parse(
            r#"<A _post="p" _while="w" name="" _skipIf="s" _onHalted="h" _uid="3" _x="1" a.b="2" b="{x}"/>"#,
        )
        .unwrap();
        let a = NodeView::new(&doc, doc.document_element().unwrap()).unwrap();
        assert_eq!(
            a.pre_conditions().collect::<Vec<_>>(),
            vec![(PreCondition::SkipIf, "s"), (PreCondition::While, "w")]
        );
        assert_eq!(
            a.post_conditions().collect::<Vec<_>>(),
            vec![(PostCondition::OnHalted, "h"), (PostCondition::Post, "p")]
        );
        // An empty name attribute is the instance name.
        assert_eq!(a.instance_name(), Some(""));
        assert_eq!(a.uid_attribute(), Some("3"));
        assert_eq!(a.autoremap(), None);
        let classes: Vec<AttributeClass> = a.attributes().map(|x| x.class).collect();
        assert_eq!(
            classes,
            vec![
                AttributeClass::PostCondition(PostCondition::Post),
                AttributeClass::PreCondition(PreCondition::While),
                AttributeClass::Name,
                AttributeClass::PreCondition(PreCondition::SkipIf),
                AttributeClass::PostCondition(PostCondition::OnHalted),
                AttributeClass::RuntimeMeta(crate::bt::RuntimeMeta::Uid),
                AttributeClass::OtherUnderscore,
                AttributeClass::NotAPort,
                AttributeClass::Port,
            ]
        );
        assert_eq!(a.ports().collect::<Vec<_>>(), vec![("b", "{x}")]);
    }

    #[test]
    fn bad_autoremap() {
        let doc = Document::parse(r#"<SubTree ID="S" _autoremap="yes"/>"#).unwrap();
        let s = NodeView::new(&doc, doc.document_element().unwrap()).unwrap();
        assert_eq!(s.autoremap(), Some(Err("yes")));
        assert!(NodeView::new(&doc, doc.root()).is_none());
    }
}
