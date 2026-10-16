# FastTick design notes

This file gives the internal architecture. The plan (`docs/PLAN.md`) gives the
scope and the milestones. Read the plan first.

## 1. Layers

```text
fasttick-app        GUI only: panels, canvas, input, dialogs
   |      |      |
   |      |   fasttick-log        .btlog / SQLite, replay index, stats, export
   |   fasttick-protocol          publisher client, LiveSession thread
   |      |      |
fasttick-core       xml (lossless DOM) -> bt (BT.CPP view) -> models, edit,
                    validate, layout, search, project; status; runtime
```

Rules:

- The GUI never parses or writes XML, `.btlog` data or protocol messages.
  It calls the library crates.
- The GUI never changes an XML document directly. It sends an edit command to
  `fasttick_core::edit`. This gives undo and redo for all changes.
- Library calls that can take time (network, large files) run on a background
  thread. Results go to the GUI through channels.

## 2. Core: documents and trees

### 2.1 Lossless XML (`xml`)

`xml::Document` is an arena of nodes (`NodeId`). It keeps all bytes of the
source: attribute order, quotes, whitespace, comments, entity references.
An unchanged document writes back byte for byte. Detached nodes can stay in
the arena. They are not written.

### 2.2 BT.CPP view (`bt`)

`bt` reads BT.CPP v4 concepts from an `xml::Document`. It does not copy the
data. It gives:

- `BtFile`: one XML file. It holds the `xml::Document`, the file path and a
  derived `BtIndex`. The index has the format version (`BTCPP_format`),
  `main_tree_to_execute`, the `<BehaviorTree>` elements, the `<include>`
  elements and the local `<TreeNodesModel>`. The index is built again after
  each edit (this is fast: a scan of the elements).
- `NodeView`: a read view of one tree node element. It gives the type ID
  (element name, or the `ID` attribute for the explicit form
  `<Action ID="X">`), the instance name (`name`), the port attributes, the
  condition attributes and the other attributes.
- Attribute classes, from the BT.CPP source:
  - reserved: `name`, `ID`, `_autoremap` (`IsReservedAttribute` in
    `basic_types.cpp`);
  - pre-conditions: `_failureIf`, `_successIf`, `_skipIf`, `_while`;
  - post-conditions: `_onHalted`, `_onFailure`, `_onSuccess`, `_post`;
  - runtime metadata: `_uid`, `_fullpath` (only in executor output);
  - other names that start with `_`: unknown, kept as they are;
  - all other attributes: ports (or SubTree remappings).
- Port values: a literal, a blackboard reference `{key}`, or `{=}` (same name
  as the port). `PortValue::parse` gives the kind.
- Annotations (`bt::annotations`): notes and groups are attributes that
  BT.CPP ignores, so they stay in the XML file. A node note is `_note` on a
  tree node. A group is `_group="<id>"` on each member, with `_group_<id>`
  (title), `_group_<id>_description` and `_group_<id>_color` on the
  `<BehaviorTree>` element. A canvas note is `_note_<id>` (text) and
  `_note_<id>_box` (`"x y width height"`) on the `<BehaviorTree>` element.
  BT.CPP 4.9 puts a node attribute that starts with `_` and is not
  reserved into `other_attributes` (a `<SubTree>` skips it), and reads
  only `ID` from a `<BehaviorTree>` element. The edit commands
  (`edit::annotations`) keep groups tidy: a moved node stays in its group
  only next to another member (or with the whole group), and a group
  without members goes away.

### 2.3 Project (`project`)

A `Project` holds several `BtFile`s and the node models. A project file
(`*.ftproj`, JSON) stores:

```json
{
  "version": 1,
  "name": "demo",
  "files": ["trees/main.xml", "trees/sub.xml"],
  "model_files": ["models/custom_models.xml"],
  "layout": {
    "trees/main.xml#MainTree": {
      "direction": "top_down",
      "structure_hash": "…",
      "positions": { "0": [0.0, 0.0], "0/1": [120.0, 80.0] }
    }
  },
  "unconnected": [
    { "file": "trees/main.xml", "tree": "MainTree", "xml": "<Sleep msec=\"10\"/>", "position": [300.0, 40.0] }
  ]
}
```

- Paths are relative to the project file.
- A node position key is the child-index path from the tree root
  (`"0/1"` = second child of the root). The `structure_hash` detects external
  edits. If the hash does not match, FastTick ignores the stored positions and
  uses the automatic layout.
- Unconnected nodes: BT XML cannot store a node without a parent. FastTick
  keeps them as detached nodes in the `xml::Document` and saves them in the
  project file (decision D5). The validator reports `UnconnectedNode`.
- A user can open a single XML file without a project. FastTick then makes an
  in-memory project with that file.

### 2.4 Node models (`models`)

`NodeModel { id, kind, ports, metadata, source }`. `PortModel { name,
direction, type_name, default, description }`. A port is *required* when it
is an input or in/out port without a default value.

Model look-up order (the first match wins):

1. the `<TreeNodesModel>` of the same file and of the files that it includes;
2. the project model files (`writeTreeNodesModelXML` output, or models that the
   user made in FastTick);
3. the built-in models of BT.CPP 4.9.0 (embedded resource);
4. for a `SubTree`: a model made from the target `<BehaviorTree>`.

### 2.5 Edits (`edit`)

`Edit` is an enum of invertible operations on one `BtFile`: insert a node or a
fragment, remove, move (re-parent or re-order), set or remove an attribute,
rename an element, add or remove a tree, change the model section. Each `apply`
returns the inverse `Edit`. A `Transaction` is a list of edits with a label.
`History` keeps undo and redo stacks and a "saved" mark (for the dirty flag).

High-level commands build transactions: add node from the palette, delete
selection, paste, duplicate, create SubTree from selection, connect,
disconnect, reorder, rename tree.

### 2.6 Validation (`validate`)

`Validator::run(&Project) -> Vec<Problem>`. A `Problem` has a severity, a
code, the file, the tree ID, the node (`NodeId`) and a message. The codes are
in `docs/PLAN.md` section 4.

### 2.7 Layout (`layout`)

Input: a tree of node sizes (the GUI measures the text). Output: a position for
each node. Algorithm: tidy tree with variable node sizes, linear or
`O(n log n)` time. Directions: top-down and left-right.

Groups: the GUI gives each card extra space (`canvas::annotate::padding`):
the margin around the members and the header above them (on the depth
axis for whole levels, because the layout is layered). Then it runs up to
four more passes: the header heights for the new box widths, and more
space where a card of another node is still in a box
(`canvas::annotate::push_out`).

## 3. GUI

### 3.1 One canvas for three modes

The canvas draws a `CanvasTree`: nodes with a label, a type, a category,
children, port lines, and decorations (status colour, previous status,
breakpoint marker, substitution marker, problem marker, search match). Three
sources build a `CanvasTree`:

- Edit mode: a tree of a `BtFile` (editable).
- Live mode: the `RuntimeTree` from the executor (read-only), with status from
  `SessionEvent::Status`.
- Replay mode: the `RuntimeTree` of a log (read-only), with status from
  `ReplayIndex::state_at`.

The canvas draws only the nodes in the visible rectangle. It caches text
layout. It must stay smooth at 1,000 and more nodes.

### 3.2 Threads

- UI thread: egui.
- Live: `LiveSession` thread (in `fasttick-protocol`). The GUI sends commands
  and reads events with `try_recv` each frame. The session calls a waker that
  requests a repaint.
- File loading of large logs: a worker thread.

### 3.3 Visual design

FastTick uses its own visual design: a neutral colour scheme with one accent
colour, painter-drawn icons, and status colours that people with a colour
vision deficiency can also tell apart (each status has a colour and a shape).
