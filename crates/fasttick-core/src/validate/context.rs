//! Data about the whole project that the file checks share: the include
//! closures, the tree IDs and the SubTree cycles.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::project::{FileId, FileRole, Project, TreeRef};
use crate::xml::NodeId;

/// Where the tree of a SubTree reference (or of `main_tree_to_execute`) is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Resolved {
    /// In the file or in a file that it includes.
    InClosure(TreeRef),
    /// Only in another project file.
    Elsewhere(TreeRef),
    /// No project file has the tree.
    Missing,
}

impl Resolved {
    /// The tree, if one is found.
    pub(super) fn tree(self) -> Option<TreeRef> {
        match self {
            Resolved::InClosure(t) | Resolved::Elsewhere(t) => Some(t),
            Resolved::Missing => None,
        }
    }
}

/// The cycle that a `<SubTree>` element closes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Cycle {
    /// The SubTree refers to its own tree.
    SelfRef,
    /// The tree IDs of the cycle: the tree of the SubTree first and last.
    Path(Vec<String>),
}

/// The shared data of one validation run.
#[derive(Debug)]
pub(super) struct Context<'p> {
    project: &'p Project,
    files: Vec<FileId>,
    /// The include closure of each file (the file first).
    closures: HashMap<FileId, Vec<FileId>>,
    /// The files that another project file includes.
    included: HashSet<FileId>,
    /// For each file: tree ID to the index of the last tree with that ID.
    tree_ids: HashMap<FileId, HashMap<&'p str, usize>>,
    /// All trees with an explicit, non-empty ID, by ID, in project order.
    trees_by_id: HashMap<&'p str, Vec<TreeRef>>,
    /// The cycles, by file and `<SubTree>` element.
    cycles: HashMap<(FileId, NodeId), Cycle>,
}

impl<'p> Context<'p> {
    pub(super) fn new(project: &'p Project) -> Context<'p> {
        let files = project.file_ids();
        let mut closures = HashMap::with_capacity(files.len());
        let mut included = HashSet::new();
        let mut tree_ids = HashMap::with_capacity(files.len());
        let mut trees_by_id: HashMap<&'p str, Vec<TreeRef>> = HashMap::new();
        for &file in &files {
            let closure = project.include_closure(file);
            included.extend(closure.iter().skip(1).copied());
            closures.insert(file, closure);
            let mut ids = HashMap::new();
            if let Some(bt) = project.file(file) {
                for (index, tree) in bt.trees().iter().enumerate() {
                    ids.insert(tree.id.as_str(), index);
                    if tree.has_explicit_id && !tree.id.is_empty() {
                        trees_by_id
                            .entry(tree.id.as_str())
                            .or_default()
                            .push(TreeRef { file, index });
                    }
                }
            }
            tree_ids.insert(file, ids);
        }
        let mut ctx = Context {
            project,
            files,
            closures,
            included,
            tree_ids,
            trees_by_id,
            cycles: HashMap::new(),
        };
        ctx.cycles = ctx.find_cycles();
        ctx
    }

    pub(super) fn project(&self) -> &'p Project {
        self.project
    }

    /// The live files, in project order.
    pub(super) fn files(&self) -> &[FileId] {
        &self.files
    }

    /// The file and the files that it includes (transitively).
    pub(super) fn closure(&self, file: FileId) -> &[FileId] {
        self.closures.get(&file).map_or(&[], Vec::as_slice)
    }

    /// True if another project file includes this file.
    pub(super) fn is_included(&self, file: FileId) -> bool {
        self.included.contains(&file)
    }

    /// True if the file is a tree file that the project file lists.
    pub(super) fn is_listed_tree_file(&self, file: FileId) -> bool {
        self.project.role(file) == Some(FileRole::Tree)
    }

    /// The trees with an explicit ID, by ID, in project order.
    pub(super) fn trees_with_id(&self, id: &str) -> &[TreeRef] {
        self.trees_by_id.get(id).map_or(&[], Vec::as_slice)
    }

    /// The index of the last tree with this ID in a file.
    fn tree_in_file(&self, file: FileId, id: &str) -> Option<TreeRef> {
        self.tree_ids
            .get(&file)?
            .get(id)
            .map(|&index| TreeRef { file, index })
    }

    /// Finds the tree with this ID for a reference in `file`: first in the
    /// file and the files that it includes, in the order of
    /// [`Project::include_closure`], then in all project files, in project
    /// order. (The same order as [`Project::find_tree_from`].)
    pub(super) fn resolve(&self, file: FileId, id: &str) -> Resolved {
        if let Some(tree) = self
            .closure(file)
            .iter()
            .find_map(|&f| self.tree_in_file(f, id))
        {
            return Resolved::InClosure(tree);
        }
        match self.files.iter().find_map(|&f| self.tree_in_file(f, id)) {
            Some(tree) => Resolved::Elsewhere(tree),
            None => Resolved::Missing,
        }
    }

    /// The tree IDs of the file and of the files that it includes, without
    /// duplicates (the trees that BT.CPP registers when it loads the file).
    pub(super) fn closure_tree_ids(&self, file: FileId) -> HashSet<&'p str> {
        let mut ids = HashSet::new();
        for f in self.closure(file) {
            if let Some(bt) = self.project.file(*f) {
                ids.extend(bt.trees().iter().map(|t| t.id.as_str()));
            }
        }
        ids
    }

    /// The cycle that a `<SubTree>` element closes, if any.
    pub(super) fn cycle(&self, file: FileId, node: NodeId) -> Option<&Cycle> {
        self.cycles.get(&(file, node))
    }

    fn tree_name(&self, tree: TreeRef) -> &'p str {
        self.project.tree(tree).map_or("", |t| t.id.as_str())
    }

    /// Finds the `<SubTree>` elements that are part of a cycle of trees.
    ///
    /// The graph has one vertex for each tree and one edge for each
    /// `<SubTree>` element in the tree (to the tree that [`Self::resolve`]
    /// finds). An edge is on a cycle if both ends are in the same strongly
    /// connected component (Tarjan's algorithm, without recursion).
    fn find_cycles(&self) -> HashMap<(FileId, NodeId), Cycle> {
        let mut vertex: HashMap<TreeRef, usize> = HashMap::new();
        let mut trees: Vec<TreeRef> = Vec::new();
        for &file in &self.files {
            if let Some(bt) = self.project.file(file) {
                for index in 0..bt.trees().len() {
                    let tree = TreeRef { file, index };
                    vertex.insert(tree, trees.len());
                    trees.push(tree);
                }
            }
        }
        // Edges: (from, to, file, element).
        let mut edges: Vec<(usize, usize, FileId, NodeId)> = Vec::new();
        let mut adjacency: Vec<Vec<usize>> = vec![Vec::new(); trees.len()];
        for (from, tree) in trees.iter().enumerate() {
            let Some(bt) = self.project.file(tree.file) else {
                continue;
            };
            let Some(entry) = bt.trees().get(tree.index) else {
                continue;
            };
            let doc = bt.doc();
            for node in doc.descendants(entry.element).skip(1) {
                let Some(element) = doc.element(node) else {
                    continue;
                };
                if element.name() != "SubTree" {
                    continue;
                }
                let Some(id) = element.attribute("ID").filter(|id| !id.is_empty()) else {
                    continue;
                };
                if let Some(to) = self
                    .resolve(tree.file, id)
                    .tree()
                    .and_then(|t| vertex.get(&t).copied())
                {
                    edges.push((from, to, tree.file, node));
                    adjacency[from].push(to);
                }
            }
        }
        let component = strongly_connected_components(&adjacency);
        let mut cycles = HashMap::new();
        for &(from, to, file, node) in &edges {
            if from == to {
                cycles.insert((file, node), Cycle::SelfRef);
            } else if component[from] == component[to] {
                let path = shortest_path(&adjacency, &component, to, from);
                let mut names = vec![self.tree_name(trees[from]).to_owned()];
                names.extend(path.iter().map(|&v| self.tree_name(trees[v]).to_owned()));
                cycles.insert((file, node), Cycle::Path(names));
            }
        }
        cycles
    }
}

/// Tarjan's algorithm without recursion. Returns the component number of
/// each vertex.
fn strongly_connected_components(adjacency: &[Vec<usize>]) -> Vec<usize> {
    const UNSEEN: usize = usize::MAX;
    let n = adjacency.len();
    let mut index = vec![UNSEEN; n];
    let mut low = vec![0; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut component = vec![UNSEEN; n];
    let mut next_index = 0;
    let mut next_component = 0;
    // The call stack: (vertex, position in its adjacency list).
    let mut calls: Vec<(usize, usize)> = Vec::new();
    for start in 0..n {
        if index[start] != UNSEEN {
            continue;
        }
        calls.push((start, 0));
        index[start] = next_index;
        low[start] = next_index;
        next_index += 1;
        stack.push(start);
        on_stack[start] = true;
        while let Some(&mut (v, ref mut i)) = calls.last_mut() {
            if let Some(&w) = adjacency[v].get(*i) {
                *i += 1;
                if index[w] == UNSEEN {
                    index[w] = next_index;
                    low[w] = next_index;
                    next_index += 1;
                    stack.push(w);
                    on_stack[w] = true;
                    calls.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
                continue;
            }
            calls.pop();
            if let Some(&(parent, _)) = calls.last() {
                low[parent] = low[parent].min(low[v]);
            }
            if low[v] == index[v] {
                while let Some(w) = stack.pop() {
                    on_stack[w] = false;
                    component[w] = next_component;
                    if w == v {
                        break;
                    }
                }
                next_component += 1;
            }
        }
    }
    component
}

/// The shortest path from `from` to `to` inside one component, with both
/// ends (breadth-first search). `from` and `to` must be in the same
/// component.
fn shortest_path(
    adjacency: &[Vec<usize>],
    component: &[usize],
    from: usize,
    to: usize,
) -> Vec<usize> {
    let mut previous: HashMap<usize, usize> = HashMap::new();
    let mut queue = VecDeque::from([from]);
    let mut seen = HashSet::from([from]);
    while let Some(v) = queue.pop_front() {
        if v == to {
            break;
        }
        for &w in &adjacency[v] {
            if component[w] == component[from] && seen.insert(w) {
                previous.insert(w, v);
                queue.push_back(w);
            }
        }
    }
    let mut path = vec![to];
    let mut current = to;
    while current != from {
        match previous.get(&current) {
            Some(&p) => {
                path.push(p);
                current = p;
            }
            // Not reachable: cannot happen inside one component.
            None => break,
        }
    }
    path.reverse();
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn components() {
        // 0 -> 1 -> 2 -> 0, 2 -> 3, 3 -> 3, 4 alone.
        let adjacency = vec![vec![1], vec![2], vec![0, 3], vec![3], vec![]];
        let c = strongly_connected_components(&adjacency);
        assert_eq!(c[0], c[1]);
        assert_eq!(c[1], c[2]);
        assert_ne!(c[2], c[3]);
        assert_ne!(c[3], c[4]);
        assert_eq!(shortest_path(&adjacency, &c, 1, 0), vec![1, 2, 0]);
        assert_eq!(shortest_path(&adjacency, &c, 0, 0), vec![0]);
    }

    #[test]
    fn components_of_a_long_chain() {
        // A chain of 100,000 vertices with one back edge: no recursion.
        let n = 100_000;
        let mut adjacency: Vec<Vec<usize>> = (0..n).map(|i| vec![i + 1]).collect();
        adjacency[n - 1] = vec![0];
        let c = strongly_connected_components(&adjacency);
        assert!(c.iter().all(|&x| x == c[0]));
        assert_eq!(shortest_path(&adjacency, &c, 1, 0).len(), n);
    }
}
