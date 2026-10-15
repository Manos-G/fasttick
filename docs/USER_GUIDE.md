# FastTick user guide

FastTick is an editor, live debugger and log replayer for BehaviorTree.CPP 4
(BT.CPP) trees. This guide tells you how to use it.

## 1. Install and start

### 1.1 Install

Use one of these procedures:

- Debian or Ubuntu: install the package.
  `sudo apt install ./fasttick_<version>_amd64.deb`
- Other Linux systems: make the AppImage executable and run it.
  `chmod +x FastTick-<version>-x86_64.AppImage`
- From the source code: run `cargo build --release`. The program is
  `target/release/fasttick`.

### 1.2 Start

Run `fasttick`, or select FastTick in the application menu.

You can give files on the command line:

```sh
fasttick tree.xml                    # a tree file (edit mode)
fasttick project.ftproj              # a project (edit mode)
fasttick run.btlog                   # a log (replay mode)
fasttick --mode live --connect localhost:1667
```

Options:

| Option | Meaning |
|---|---|
| `--theme THEME` | The theme for this session (see 2.1). |
| `--mode edit\|live\|replay` | The mode at the start. |
| `--connect HOST[:PORT]` | Connect to an executor. The default port is 1667. |
| `--help`, `--version` | Show the help text or the version. |

## 2. The main window

Each mode starts with a **Start** tab. It shows the main actions of the
mode, the recent files (the files of the mode first; click a name to open
it), what is new in this version, and some tips.

The window has three modes. Use the switch at the top, or push Ctrl+1,
Ctrl+2 or Ctrl+3.

- **Edit**: write and change tree files.
- **Live**: watch and debug a running executor.
- **Replay**: replay a log file.

Each mode has its own panels. You can move a panel: drag its tab to a new
position. Use **View** to show a closed panel again.

The canvas shows one tree. These actions are the same in all modes:

| Action | Result |
|---|---|
| Click | Select a node. |
| Shift+click, Ctrl+click | Add a node to the selection, or remove it. |
| Drag on the background | Select the nodes in a rectangle. |
| Mouse wheel, touchpad | Pan. |
| Ctrl+wheel, pinch | Zoom at the pointer. You cannot zoom out more than the size that shows the whole tree. |
| Middle drag, Space+drag | Pan. |
| Double-click a SubTree | Expand it (show its tree) or collapse it. |
| Double-click another node with children | Fold it (hide the nodes below it) or unfold it. A folded node shows the number of hidden nodes, and the marker of the most serious problem below it. Right-click > **Unfold all** shows all nodes again. |
| Arrow keys | Go to the parent, a child or a sibling. |
| F | Fit the tree to the view. |
| Shift+F | Fit the selected nodes to the view (at most 100 %). |
| Ctrl+L | Change the layout: top to bottom, or left to right. |
| Ctrl+\ | Split the view (a second tab of the same document). |

Push F1 to see all keyboard shortcuts.

Put the pointer on a node to see a short description of its type, all its
ports and its conditions. The description comes from the model of the
type: FastTick has a text for each built-in node of BT.CPP, and it uses
the `description` metadata of your own models. The **Palette** shows the
same description, and the description of each port.

Each node card shows all its ports and remappings. A long value (for
example the script of a `Script` node) continues on the next lines, and the
card becomes taller. A value has a maximum of 12 lines on the card; the
Inspector and the tooltip show all of it.

### 2.1 Themes

Select **View** > **Theme**. These themes are available:

| Group | Themes | `--theme` value |
|---|---|---|
| FastTick | Light, Dark, System | `light`, `dark`, `system` |
| OpenWire | OpenWire Light, OpenWire Dark | `openwire-light`, `openwire-dark` |
| Catppuccin | Latte, Frappé, Macchiato, Mocha | `catppuccin-latte`, `catppuccin-frappe`, `catppuccin-macchiato`, `catppuccin-mocha` |
| VS Code | Dark Modern, Dark+, Light Modern | `vscode-dark-modern`, `vscode-dark-plus`, `vscode-light-modern` |

**System** uses FastTick Light or FastTick Dark, as your system setting.
FastTick keeps the theme in its settings.

### 2.2 Search commands

Push Ctrl+Shift+P (or select **View** > **Search commands…**) to find an
action by its name. Type some letters of the name: the letters must be in
the same order, but they need not be side by side. The list shows:

- the commands that you can use now, with their shortcuts;
- the trees of the active document;
- the recent files that exist;
- the themes.

Use the Up and Down arrows to select an item. Push Enter or click the item
to use it. Push Escape or click outside the list to close it.

Push Ctrl+P (**View** > **Go to tree…**) to see only the trees of the
active document. For example, type `nav` and push Enter to show the tree
`Navigate`.

### 2.3 Export an image of a tree

Select **File** > **Export tree as SVG…** and give a file name. FastTick
writes the tree of the active tab as an SVG image, in the colours of the
current theme. The image shows all nodes of the tree in the tab (also the
nodes outside the view), as they show at 100 % zoom. In live and replay
mode, the image also shows the statuses of the nodes.

The image does not show the selection, the search results, the
breakpoints or the problem markers.

## 3. Edit mode

### 3.1 Open, make and save files

- **File** > **Open project or XML…** (Ctrl+O) opens a tree file (`.xml`) or
  a project (`.ftproj`).
- **File** > **New project** (Ctrl+N) makes a new file with one empty tree,
  `MainTree`.
- **File** > **Save** (Ctrl+S) saves all changed files. A new file asks for
  a name.
- **File** > **Save as…** (Ctrl+Shift+S) saves the main file with a new name.

You can also drag files from the file manager onto the FastTick window:
tree files and projects open in edit mode, logs in replay mode.

A tab with changes that are not saved has a dot (•) after its name. When
you close such a tab, or close FastTick, FastTick asks if it must save the
changes: **Save** (or **Save all**), **Do not save** or **Cancel**.

FastTick keeps the text of a file. It changes only the elements and the
attributes that you change. Comments, the order of attributes and the
white space stay.

### 3.2 Projects with more than one file

A project lists one or more tree files and model files. Use the **Project**
menu in the **Trees** panel:

- **Add tree files…** adds XML files to the project. You can select more
  than one file.
- **New tree file…** makes a new XML file with one tree.
- **Add model files…** adds files with node models (see 3.6).
- **Add missing files from the folder** finds the XML files that have the
  missing SubTrees and node models, and adds them (see below).
- **Save project as…** writes the project file (`.ftproj`).

A tree often uses SubTrees that are in other files of the same folder,
without an `<include>`. The executor loads all these files. When you open
such a tree, FastTick looks at the XML files in the same folder. If they
have the missing trees, a notice shows the file names. Click **Add the
files** to add them to the project. FastTick also adds the files that the
new files need (for example a library that uses other libraries). A file
with only a `<TreeNodesModel>` becomes a model file when it has a missing
node model.

Example: `B1_localize.xml` uses the SubTrees `GetMaterial` and
`LeaveMaterial` (in `SubTrees_library.xml`) and `Reset`, `SideArmsOpen` and
`SideArmsClose` (in `Automation_library.xml`). Do these steps:

1. Open `B1_localize.xml`.
2. Click **Add the files** in the notice, or select **Project** > **Add
   missing files from the folder**.
3. Select **Project** > **Save project as…** and save the project in the
   same folder.

Next time, open the `.ftproj` file. It opens all the files together.

The **Trees** panel shows the trees of each file. Click a tree to show it.
The word "main" marks the `main_tree_to_execute` of each file.

To add, rename or delete a tree, use **Add tree…** or right-click the tree.
**Set as main tree** changes `main_tree_to_execute`.

### 3.3 Add, move and delete nodes

Each node that can have one more child (a control node, or a decorator
without a child) has a dot with a plus sign at the middle of its bottom
edge (right edge in the left-to-right layout).

- Drag from the dot. A line follows the pointer. Release it where the new
  node must go: left of a child to add the node before that child, or
  below the last child to add it as the last child. A search window opens.
  Type some letters of the type (for example `rsq` for
  `ReactiveSequence`), use the arrow keys to select, and push Enter. Push
  Escape, or click outside the window, to cancel.
- Release the line on an unconnected node to connect that node as the last
  child.
- Click the dot to add a last child with the same search window.
- Select a node and push Shift+A (or select **Edit** > **Add node…**).
  The new node is a child of a control node or an empty decorator, else a
  sibling after the selected node. Without a selection, the new node is
  unconnected.

Other ways to add nodes:

- Drag a node type from the **Palette** onto the canvas. Drop it:
  - on the middle of a node to add it as the last child,
  - on the left or right end of a node to add it before or after the node
    (top and bottom ends in the left-to-right layout),
  - on an empty place to add an unconnected node.
- Double-click a type in the palette to add it to the selected node.
- Drag a node to move it with its children. You do not have to drop on a
  card exactly: a drop near a card (for example between two siblings)
  goes to the nearest card, as if it were on the nearest end of that card.
  Only a drop far from all cards disconnects the node. The label next to
  the pointer tells where the node goes. Push Escape during the drag to
  cancel the move.
- Right-click a node to see the edit actions: **Add child…**, **Add sibling
  before…**, **Add sibling after…** (these open the search window),
  **Cut**, **Copy**, **Duplicate**,
  **Delete**, **Paste as child**, **Move up**, **Move down**, **Disconnect**
  and **Create SubTree from selection…**.
- Right-click an empty place to add or paste an unconnected node.
- Ctrl+C copies the selected nodes, Ctrl+X cuts them and Ctrl+V pastes
  them. The paste goes into the selected node as its last children. If
  the selected node cannot take more children (for example an action),
  the paste goes after it. Without a selection, the paste makes
  unconnected nodes. FastTick puts the XML of the copied nodes on the
  system clipboard. You can also paste the XML of nodes that you copied
  in another program, for example `<Sleep msec="100"/>`.

Unconnected nodes stay in the project file. BT.CPP does not see them.

A drop that BT.CPP does not allow (for example a second child of a
decorator) shows a notice. The tree does not change.

### 3.4 Change a node

Select a node. The **Inspector** shows its properties:

- **Name**: the `name` attribute. An empty name removes the attribute.
- **Type**: select another type of the same kind. The attributes and the
  children stay.
- **Ports**: one field for each port of the model, the full width of the
  panel, below the port name, its type and its description. A long value
  continues on the next lines. Push Enter to end the edit. A bold name and
  a star mark a required port. The hint text shows the default value.
  Click **{ }** to change between a literal value and a blackboard key.
  Click **keys** to select one of the blackboard keys that the tree uses.
  Click **×** to remove the attribute. Use **Add port** for a port that the
  model does not have.
- **Ports and remappings** (SubTree nodes): the ports of the model of the
  tree, the remappings of the node, and the blackboard keys that the tree
  uses ("key of the tree"). Type a value or `{key}` in the field of a key
  to connect it.
- **Remapping** (SubTree nodes): **Automatic (_autoremap)**.
- **Pre- and post-conditions**: one script for each condition. An empty
  script removes the condition. FastTick checks each script with the
  grammar of BT.CPP 4.9.0. If BT.CPP cannot parse a script, the message of
  BT.CPP shows below the field and in the problems list, and the node gets
  an error marker. (BT.CPP does not load a tree with such a script.)
- **Problems**: the problems of the node.

Each change is one undo step. The keystrokes in one field become one step.
Use **Edit** > **Undo** (Ctrl+Z) and **Redo** (Ctrl+Y or Ctrl+Shift+Z).

The node cards show the pre- and post-conditions below the ports, in a
box with a solid purple outline and a light purple background, in all
three modes. The Inspector and the tooltip use the same box. In live and replay
mode, the Inspector shows them read-only.

FastTick also checks the scripts in the `code` port of `Script` and
`ScriptCondition` nodes and in the `if` port of `Precondition` nodes,
because BT.CPP parses them when it creates these nodes.

### 3.5 SubTrees

- Select sibling nodes. Right-click them and select **Create SubTree from
  selection…**. Type a tree ID. FastTick moves the nodes to a new tree and
  puts a SubTree node at their position. It remaps the blackboard keys that
  the nodes use.
- Right-click a SubTree node and select **Open tree** to edit its tree.
- Double-click a SubTree node to show its tree inline. The inline nodes are
  read-only.

### 3.6 Node models

The **Palette** shows the node types that the file can use: the built-in
types of BT.CPP 4.9.0, the models in `<TreeNodesModel>` sections and the
models in project model files.

- Click **New…** to add a model to the file.
- Right-click a model and select **Edit model…** to change its ID, kind and
  ports. Built-in models are read-only.

To use the models of your own nodes:

1. Export them with `BT::writeTreeNodesModelXML` to an XML file.
2. In the **Trees** panel, select **Project** > **Add model files…**.
3. Select the XML file.

The types show in the palette, and the "not known" problems go away. To
keep the model file in the project, select **Project** > **Save project
as…**. You can also copy the `<TreeNodesModel>` into a tree file.

### 3.7 Problems, XML and search

- The **Problems** panel lists the problems of all files. Click a problem to
  show the node. The buttons filter errors, warnings and notes.
- The **XML** panel shows the text of the file. The element of the selected
  node has a coloured background. The panel is read-only.
- The **Search** panel (Ctrl+F) finds nodes by name, type, port name, port
  value or condition script. Use **Aa** for match case, **Word** for whole
  words and **\* ?** for wildcards. The canvas marks the matching nodes.

## 4. Live mode

The executor must run the BT.CPP publisher:

```cpp
BT::Groot2Publisher publisher(tree, 1667);
```

### 4.1 Connect

1. Type the host and the port in the live bar.
2. Click **Connect** (F5).

The canvas shows the tree of the executor and the status of each node. Use
**Poll** to set the time between two status reads. Click **Disconnect**
(Shift+F5) to stop.

### 4.2 Breakpoints

1. Right-click a node.
2. Select **Add breakpoint before the node** (or push F9).

When the tree comes to the node, the executor stops. A banner shows the
node. Select one of these actions:

- **Continue** (F8): the node ticks.
- **Force SUCCESS** or **Force FAILURE**: the node does not tick. It returns
  this status.

The **Breakpoints** panel lists all breakpoints. Use **Save…** and
**Load…** to keep breakpoints in a file.

Note: BT.CPP 4.9.0 cannot stop after a node, so this action is not
available.

### 4.3 Forced results

Right-click a node and select **Force SUCCESS** or **Force FAILURE**. The
node does not tick. It returns the status at each tick. Select **Clear
forced result** to stop. The canvas marks these nodes.

If FastTick disconnects or stops, the executor removes all breakpoints and
forced results when its heartbeat time (5 seconds in BT.CPP 4.9.0) ends.

### 4.4 Blackboards

The **Blackboard** panel shows the values of the watched blackboards. Select
the blackboards to watch. Use the filter to find keys and values.

### 4.5 Recording

Click **Record** (Ctrl+R) in the live bar. FastTick records the
transitions. Click **Stop recording** and save the `.btlog` file. Open it in
replay mode.

## 5. Replay mode

Open a log with **File** > **Open log…** (Ctrl+Shift+O): a `.btlog` file
(`BT::FileLogger2`) or a SQLite log (`BT::SqliteLogger`, `.db3` or `.btdb`).

- **Timeline**: play or pause (P), step back (,) or forward (.), go to the
  start (Ctrl+Home) or the end (Ctrl+End). Drag the position to scrub. Set
  the speed in the list.
- **Canvas**: the status of each node at the position of the timeline.
- **Transitions**: the list of transitions. Use the filters to show some
  nodes, some statuses or a time range. Click a row to show the node. Use
  **JSON…** or **CSV…** to export the filtered rows.
- **Statistics**: for each node, the number of transitions and results and
  the time in RUNNING.

## 6. Settings and files

FastTick keeps the window layout, the theme, the recent files and the poll
interval in the settings directory of your system (for example
`~/.local/share/fasttick` on Linux).

## 7. Troubleshooting

| Problem | Cause and remedy |
|---|---|
| "Node not recognized" errors for your own nodes | FastTick has no model of the node types. Add the models (see 3.6). |
| Live mode does not connect | Make sure that the executor runs the publisher and that the port is correct. The publisher also uses the next port (port + 1). |
| The last transitions of a log are not there | `BT::FileLogger2` can lose them when the executor stops. |
