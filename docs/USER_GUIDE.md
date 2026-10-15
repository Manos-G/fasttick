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
| `--theme light\|dark\|system` | The theme for this session. |
| `--mode edit\|live\|replay` | The mode at the start. |
| `--connect HOST[:PORT]` | Connect to an executor. The default port is 1667. |
| `--help`, `--version` | Show the help text or the version. |

## 2. The main window

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
| Ctrl+wheel, pinch | Zoom at the pointer. |
| Middle drag, Space+drag | Pan. |
| Double-click a SubTree | Expand it (show its tree) or collapse it. |
| Arrow keys | Go to the parent, a child or a sibling. |
| F | Fit the tree to the view. |
| Ctrl+L | Change the layout: top to bottom, or left to right. |
| Ctrl+\ | Split the view (a second tab of the same document). |

Push F1 to see all keyboard shortcuts.

## 3. Edit mode

### 3.1 Open, make and save files

- **File** > **Open project or XML…** (Ctrl+O) opens a tree file (`.xml`) or
  a project (`.ftproj`).
- **File** > **New project** (Ctrl+N) makes a new file with one empty tree,
  `MainTree`.
- **File** > **Save** (Ctrl+S) saves all changed files. A new file asks for
  a name.
- **File** > **Save as…** (Ctrl+Shift+S) saves the main file with a new name.

FastTick keeps the text of a file. It changes only the elements and the
attributes that you change. Comments, the order of attributes and the
white space stay.

### 3.2 Projects with more than one file

A project lists one or more tree files and model files. Use the **Project**
menu in the **Trees** panel:

- **Add tree file…** adds an XML file to the project.
- **New tree file…** makes a new XML file with one tree.
- **Save project as…** writes the project file (`.ftproj`).

The **Trees** panel shows the trees of each file. Click a tree to show it.
The word "main" marks the `main_tree_to_execute` of each file.

To add, rename or delete a tree, use **Add tree…** or right-click the tree.
**Set as main tree** changes `main_tree_to_execute`.

### 3.3 Add, move and delete nodes

- Drag a node type from the **Palette** onto the canvas. Drop it:
  - on the middle of a node to add it as the last child,
  - on the left or right end of a node to add it before or after the node
    (top and bottom ends in the left-to-right layout),
  - on an empty place to add an unconnected node.
- Double-click a type in the palette to add it to the selected node.
- Drag a node to move it with its children. A drop on an empty place
  disconnects the node. Push Escape during the drag to cancel the move.
- Right-click a node to see the edit actions: **Add child**, **Add sibling
  before**, **Add sibling after**, **Cut**, **Copy**, **Duplicate**,
  **Delete**, **Paste as child**, **Move up**, **Move down**, **Disconnect**
  and **Create SubTree from selection…**.
- Right-click an empty place to add or paste an unconnected node.

Unconnected nodes stay in the project file. BT.CPP does not see them.

A drop that BT.CPP does not allow (for example a second child of a
decorator) shows a notice. The tree does not change.

### 3.4 Change a node

Select a node. The **Inspector** shows its properties:

- **Name**: the `name` attribute. An empty name removes the attribute.
- **Type**: select another type of the same kind. The attributes and the
  children stay.
- **Ports**: one row for each port of the model. A bold name and a star
  mark a required port. The hint text shows the default value. Click
  **{ }** to change between a literal value and a blackboard key. Click
  **×** to remove the attribute. Use **Add port** for a port that the model
  does not have.
- **Remapping** (SubTree nodes): **Automatic (_autoremap)**.
- **Pre- and post-conditions**: one script for each condition. An empty
  script removes the condition.
- **Problems**: the problems of the node.

Each change is one undo step. The keystrokes in one field become one step.
Use **Edit** > **Undo** (Ctrl+Z) and **Redo** (Ctrl+Y or Ctrl+Shift+Z).

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

To use the models of your own nodes, export them with
`BT::writeTreeNodesModelXML` and add the file to the project as a model
file, or copy the `<TreeNodesModel>` into a tree file.

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
