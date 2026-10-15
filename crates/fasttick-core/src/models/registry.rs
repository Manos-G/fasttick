//! The model registry: finds the model of a node type.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::bt::{BtFile, NodeView};
use crate::builtin;
use crate::project::FileId;

use super::{ModelSource, NodeKind, NodeModel};

/// A list of models with an index by ID (the first model with an ID wins).
#[derive(Debug, Clone, Default)]
struct ModelList {
    models: Vec<NodeModel>,
    by_id: HashMap<String, usize>,
}

impl ModelList {
    fn new(models: Vec<NodeModel>) -> ModelList {
        let mut by_id = HashMap::with_capacity(models.len());
        for (i, m) in models.iter().enumerate() {
            by_id.entry(m.id.clone()).or_insert(i);
        }
        ModelList { models, by_id }
    }

    fn get(&self, id: &str) -> Option<&NodeModel> {
        self.by_id.get(id).map(|&i| &self.models[i])
    }
}

/// One file as input for [`ModelRegistry::rebuild`].
#[derive(Debug, Clone, Copy)]
pub struct RegistryFile<'a> {
    /// The file ID in the project.
    pub id: FileId,
    /// True for a project model file, false for a tree file (or an included
    /// file).
    pub is_model_file: bool,
    /// The file.
    pub file: &'a BtFile,
    /// The look-up order of the file: the file itself, then the files that
    /// it includes (transitively, depth first, each file once).
    pub closure: &'a [FileId],
}

/// Finds node models in the look-up order of `docs/DESIGN.md` section 2.4.
///
/// The first match wins:
///
/// 1. the `<TreeNodesModel>` of the file, then of the files that it
///    includes ([`ModelSource::File`]);
/// 2. the project model files, in project order
///    ([`ModelSource::ProjectModels`]);
/// 3. the built-in models of BT.CPP 4.9.0 ([`ModelSource::Builtin`]);
/// 4. a SubTree model made from a `<BehaviorTree>` with this ID
///    ([`ModelSource::InferredSubTree`]): first in the file and its
///    includes, then in all project files. It has no ports, because
///    BT.CPP accepts any remapping for a SubTree without a declared model.
///
/// Step 4 does not check how the node is written. A node `<Nav/>` (compact
/// form) whose ID is a tree ID gets the inferred SubTree model, but BT.CPP
/// accepts only `<SubTree ID="Nav"/>` for it. The validator checks this.
///
/// The registry holds copies of the models. [`crate::project::Project`]
/// calls [`ModelRegistry::rebuild`] after a file changes.
#[derive(Debug, Clone)]
pub struct ModelRegistry {
    builtin: ModelList,
    aliases: ModelList,
    v3_aliases: bool,
    local: HashMap<FileId, ModelList>,
    closure: HashMap<FileId, Vec<FileId>>,
    project: ModelList,
    inferred: Vec<NodeModel>,
    /// The positions in `inferred` for each tree ID, in project order.
    inferred_by_id: HashMap<String, Vec<usize>>,
}

impl Default for ModelRegistry {
    fn default() -> Self {
        ModelRegistry::new()
    }
}

impl ModelRegistry {
    /// Makes a registry with only the built-in models.
    pub fn new() -> ModelRegistry {
        let aliases = builtin::V3_ALIASES
            .iter()
            .filter_map(|(alias, target)| {
                builtin::model(target).map(|m| NodeModel {
                    id: (*alias).to_owned(),
                    ..m.clone()
                })
            })
            .collect();
        ModelRegistry {
            builtin: ModelList::new(builtin::models().to_vec()),
            aliases: ModelList::new(aliases),
            v3_aliases: false,
            local: HashMap::new(),
            closure: HashMap::new(),
            project: ModelList::new(Vec::new()),
            inferred: Vec::new(),
            inferred_by_id: HashMap::new(),
        }
    }

    /// Turns the BT.CPP 3 alias IDs on or off ([`builtin::V3_ALIASES`],
    /// for example `SequenceStar`). BT.CPP 4.9.0 registers them only when
    /// it is built with `USE_V3_COMPATIBLE_NAMES` (off by default). The
    /// default here is off.
    pub fn set_v3_aliases(&mut self, enabled: bool) {
        self.v3_aliases = enabled;
    }

    /// True if the BT.CPP 3 alias IDs are on.
    pub fn v3_aliases(&self) -> bool {
        self.v3_aliases
    }

    /// Builds the tables again from the files of a project. The order of
    /// `files` is the project order.
    pub fn rebuild<'a>(&mut self, files: impl IntoIterator<Item = RegistryFile<'a>>) {
        self.local.clear();
        self.closure.clear();
        let mut project = Vec::new();
        self.inferred.clear();
        self.inferred_by_id.clear();
        for f in files {
            let index = f.file.index();
            if f.is_model_file {
                project.extend(index.models.models.iter().map(|m| {
                    m.model
                        .clone()
                        .with_source(ModelSource::ProjectModels(f.id))
                }));
            } else {
                let models = index
                    .models
                    .models
                    .iter()
                    .map(|m| m.model.clone().with_source(ModelSource::File(f.id)))
                    .collect();
                self.local.insert(f.id, ModelList::new(models));
                self.closure.insert(f.id, f.closure.to_vec());
            }
            for tree in &index.trees {
                let mut model = NodeModel::new(tree.id.clone(), NodeKind::SubTree);
                model.source = ModelSource::InferredSubTree(f.id);
                self.inferred_by_id
                    .entry(tree.id.clone())
                    .or_default()
                    .push(self.inferred.len());
                self.inferred.push(model);
            }
        }
        self.project = ModelList::new(project);
    }

    /// The look-up order of a file: the file, then its includes.
    fn closure_of<'s>(&'s self, file: &'s FileId) -> &'s [FileId] {
        match self.closure.get(file) {
            Some(c) if !c.is_empty() => c,
            _ => std::slice::from_ref(file),
        }
    }

    /// Finds a declared model: steps 1 to 3 of the look-up order (no
    /// inferred SubTree models).
    pub fn lookup_declared(
        &self,
        file: impl Into<Option<FileId>>,
        type_id: &str,
    ) -> Option<&NodeModel> {
        if let Some(file) = file.into() {
            for f in self.closure_of(&file) {
                if let Some(m) = self.local.get(f).and_then(|l| l.get(type_id)) {
                    return Some(m);
                }
            }
        }
        self.project
            .get(type_id)
            .or_else(|| self.builtin.get(type_id))
            .or_else(|| {
                if self.v3_aliases {
                    self.aliases.get(type_id)
                } else {
                    None
                }
            })
    }

    /// Finds the SubTree model made from a `<BehaviorTree>` with this ID
    /// (step 4).
    pub fn lookup_inferred(
        &self,
        file: impl Into<Option<FileId>>,
        tree_id: &str,
    ) -> Option<&NodeModel> {
        let candidates = self.inferred_by_id.get(tree_id)?;
        if let Some(file) = file.into() {
            for f in self.closure_of(&file) {
                let found = candidates
                    .iter()
                    .map(|&i| &self.inferred[i])
                    .find(|m| m.source == ModelSource::InferredSubTree(*f));
                if found.is_some() {
                    return found;
                }
            }
        }
        candidates.first().map(|&i| &self.inferred[i])
    }

    /// Finds the model of a type ID in the full look-up order. `file` is the
    /// file that contains the node, or `None` for no file context (then
    /// step 1 is skipped).
    pub fn lookup(
        &self,
        file: impl Into<Option<FileId>>,
        type_id: &str,
    ) -> Option<(&NodeModel, ModelSource)> {
        let file = file.into();
        self.lookup_declared(file, type_id)
            .or_else(|| self.lookup_inferred(file, type_id))
            .map(|m| (m, m.source))
    }

    /// The kind of a type ID, from its model.
    pub fn kind_of(&self, file: impl Into<Option<FileId>>, type_id: &str) -> Option<NodeKind> {
        self.lookup(file, type_id).map(|(m, _)| m.kind)
    }

    /// The kind of a node element, as BT.CPP creates it:
    ///
    /// - a `<SubTree>` element is a SubTree;
    /// - else the kind of the declared model of its type ID (BT.CPP uses the
    ///   registered type and ignores the tag of the explicit form);
    /// - else, for the explicit form, the kind of the tag;
    /// - else `None` (an unknown type).
    pub fn node_kind(
        &self,
        file: impl Into<Option<FileId>>,
        node: &NodeView<'_>,
    ) -> Option<NodeKind> {
        if node.is_subtree() {
            return Some(NodeKind::SubTree);
        }
        node.type_id()
            .and_then(|id| self.lookup_declared(file, id))
            .map(|m| m.kind)
            .or_else(|| node.category_tag())
    }

    /// All models that are visible from a file (or without file context),
    /// one for each ID (the model that [`ModelRegistry::lookup`] gives),
    /// grouped by kind and sorted by ID. For the palette.
    pub fn all_models(
        &self,
        file: impl Into<Option<FileId>>,
    ) -> BTreeMap<NodeKind, Vec<&NodeModel>> {
        let file = file.into();
        let mut all: Vec<&str> = Vec::new();
        if let Some(file) = &file {
            for f in self.closure_of(file) {
                if let Some(l) = self.local.get(f) {
                    all.extend(l.models.iter().map(|m| m.id.as_str()));
                }
            }
        }
        all.extend(self.project.models.iter().map(|m| m.id.as_str()));
        all.extend(self.builtin.models.iter().map(|m| m.id.as_str()));
        if self.v3_aliases {
            all.extend(self.aliases.models.iter().map(|m| m.id.as_str()));
        }
        all.extend(self.inferred.iter().map(|m| m.id.as_str()));
        let mut seen = HashSet::new();
        all.retain(|id| seen.insert(*id));
        let mut out: BTreeMap<NodeKind, Vec<&NodeModel>> = BTreeMap::new();
        for id in all {
            if let Some((model, _)) = self.lookup(file, id) {
                out.entry(model.kind).or_default().push(model);
            }
        }
        for list in out.values_mut() {
            list.sort_by(|a, b| a.id.cmp(&b.id));
        }
        out
    }

    /// The built-in models.
    pub fn builtin_models(&self) -> &[NodeModel] {
        &self.builtin.models
    }

    /// The models of all project model files, in project order.
    pub fn project_models(&self) -> &[NodeModel] {
        &self.project.models
    }

    /// The models of the `<TreeNodesModel>` of one tree file (without its
    /// includes).
    pub fn local_models(&self, file: FileId) -> &[NodeModel] {
        self.local.get(&file).map_or(&[], |l| l.models.as_slice())
    }

    /// The SubTree models made from all `<BehaviorTree>` elements of the
    /// project, in project order.
    pub fn inferred_models(&self) -> &[NodeModel] {
        &self.inferred
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(text: &str) -> BtFile {
        BtFile::parse(text, None).unwrap()
    }

    #[test]
    fn builtin_only() {
        let reg = ModelRegistry::new();
        let (model, source) = reg.lookup(None, "Sequence").unwrap();
        assert_eq!(model.kind, NodeKind::Control);
        assert_eq!(source, ModelSource::Builtin);
        assert_eq!(reg.kind_of(None, "Inverter"), Some(NodeKind::Decorator));
        assert_eq!(reg.lookup(None, "SequenceStar"), None);
        assert_eq!(reg.lookup(FileId(3), "Nope"), None);
        let palette = reg.all_models(None);
        let total: usize = palette.values().map(Vec::len).sum();
        assert_eq!(total, 42);
        assert_eq!(palette[&NodeKind::SubTree].len(), 1);
    }

    #[test]
    fn node_kinds() {
        let file = file(
            r#"<root><BehaviorTree ID="T"><Sequence>
            <SubTree ID="Inverter"/><Action ID="Fallback"/><Action ID="Unknown"/>
            <Unknown/><Inverter/><Decorator/>
            </Sequence></BehaviorTree></root>"#,
        );
        let reg = ModelRegistry::new();
        let seq = file.node(file.trees()[0].root.unwrap()).unwrap();
        let kinds: Vec<Option<NodeKind>> =
            seq.children().map(|c| reg.node_kind(None, &c)).collect();
        assert_eq!(
            kinds,
            vec![
                Some(NodeKind::SubTree),
                Some(NodeKind::Control),
                Some(NodeKind::Action),
                None,
                Some(NodeKind::Decorator),
                Some(NodeKind::Decorator),
            ]
        );
        assert_eq!(reg.node_kind(None, &seq), Some(NodeKind::Control));
    }

    #[test]
    fn v3_aliases() {
        let mut reg = ModelRegistry::new();
        reg.set_v3_aliases(true);
        assert!(reg.v3_aliases());
        let (model, source) = reg.lookup(None, "SequenceStar").unwrap();
        assert_eq!(model.id, "SequenceStar");
        assert_eq!(model.kind, NodeKind::Control);
        assert_eq!(source, ModelSource::Builtin);
        let total: usize = reg.all_models(None).values().map(Vec::len).sum();
        assert_eq!(total, 43);
    }

    #[test]
    fn look_up_order() {
        let main = file(
            r#"<root><BehaviorTree ID="Main"><SubTree ID="Inc"/></BehaviorTree>
            <TreeNodesModel><Action ID="Sequence"/><Action ID="Local"/></TreeNodesModel></root>"#,
        );
        let inc = file(
            r#"<root><BehaviorTree ID="Inc"><AlwaysSuccess/></BehaviorTree>
            <TreeNodesModel><Condition ID="Local"/><Condition ID="FromInclude"/></TreeNodesModel></root>"#,
        );
        let other = file(
            r#"<root><BehaviorTree ID="Other"><AlwaysSuccess/></BehaviorTree>
            <BehaviorTree ID="Inc"><AlwaysFailure/></BehaviorTree>
            <TreeNodesModel><Action ID="OtherLocal"/></TreeNodesModel></root>"#,
        );
        let models = file(
            r#"<root><TreeNodesModel><Decorator ID="Shared"/><Decorator ID="FromInclude"/>
            <Action ID="Inverter"/></TreeNodesModel></root>"#,
        );
        let (m, i, o, p) = (FileId(0), FileId(1), FileId(2), FileId(3));
        let main_closure = [m, i];
        let inc_closure = [i];
        let other_closure = [o];
        let mut reg = ModelRegistry::new();
        reg.rebuild([
            RegistryFile {
                id: m,
                is_model_file: false,
                file: &main,
                closure: &main_closure,
            },
            RegistryFile {
                id: i,
                is_model_file: false,
                file: &inc,
                closure: &inc_closure,
            },
            RegistryFile {
                id: o,
                is_model_file: false,
                file: &other,
                closure: &other_closure,
            },
            RegistryFile {
                id: p,
                is_model_file: true,
                file: &models,
                closure: &[],
            },
        ]);

        // 1. Local file first, then its includes.
        assert_eq!(reg.lookup(m, "Sequence").unwrap().1, ModelSource::File(m));
        assert_eq!(reg.lookup(m, "Local").unwrap().0.kind, NodeKind::Action);
        assert_eq!(reg.lookup(i, "Local").unwrap().0.kind, NodeKind::Condition);
        assert_eq!(
            reg.lookup(m, "FromInclude").unwrap().1,
            ModelSource::File(i)
        );
        // Models of files that are not included are not visible.
        assert_eq!(reg.lookup(m, "OtherLocal"), None);
        assert_eq!(reg.lookup(o, "OtherLocal").unwrap().1, ModelSource::File(o));
        // 2. Project model files before built-ins.
        assert_eq!(
            reg.lookup(m, "Shared").unwrap().1,
            ModelSource::ProjectModels(p)
        );
        assert_eq!(
            reg.lookup(o, "FromInclude").unwrap().1,
            ModelSource::ProjectModels(p)
        );
        assert_eq!(
            reg.lookup(None, "Inverter").unwrap().0.kind,
            NodeKind::Action
        );
        // 3. Built-ins.
        assert_eq!(reg.lookup(o, "Sequence").unwrap().1, ModelSource::Builtin);
        // 4. Inferred SubTree models: the include first, then the project.
        let (inc_model, source) = reg.lookup(m, "Inc").unwrap();
        assert_eq!(inc_model.kind, NodeKind::SubTree);
        assert!(inc_model.ports.is_empty());
        assert_eq!(source, ModelSource::InferredSubTree(i));
        assert_eq!(
            reg.lookup(o, "Inc").unwrap().1,
            ModelSource::InferredSubTree(o)
        );
        assert_eq!(
            reg.lookup(None, "Inc").unwrap().1,
            ModelSource::InferredSubTree(i)
        );
        assert_eq!(
            reg.lookup(m, "Other").unwrap().1,
            ModelSource::InferredSubTree(o)
        );
        assert!(reg.lookup_declared(m, "Inc").is_none());

        assert_eq!(reg.local_models(m).len(), 2);
        assert_eq!(reg.project_models().len(), 3);
        assert_eq!(reg.inferred_models().len(), 4);
        assert_eq!(reg.builtin_models().len(), 42);

        let palette = reg.all_models(m);
        let actions: Vec<&str> = palette[&NodeKind::Action]
            .iter()
            .map(|m| m.id.as_str())
            .collect();
        assert!(actions.contains(&"Sequence"));
        assert!(actions.contains(&"Local"));
        assert!(actions.contains(&"Inverter"));
        assert!(!actions.contains(&"OtherLocal"));
        let subtrees: Vec<&str> = palette[&NodeKind::SubTree]
            .iter()
            .map(|m| m.id.as_str())
            .collect();
        assert_eq!(subtrees, vec!["Inc", "Main", "Other", "SubTree"]);
        let total: usize = palette.values().map(Vec::len).sum();
        let mut unique: Vec<&str> = palette.values().flatten().map(|m| m.id.as_str()).collect();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), total);
    }
}
