# FastTick plan (for approval)

- Status: approved on 2026-10-14. Licence: MIT. All other recommendations (D2 to D6) approved.
- Date: 2026-10-14.
- Reference library: BehaviorTree.CPP 4.9.0 (commit `3ff6a32`).
- Writing standard: ASD-STE100 (Simplified Technical English).

## 1. Project licence

Decision: **MIT** (the user selected MIT on 2026-10-14).

- MIT is compatible with BT.CPP (MIT) and libzmq (MPL-2.0).
- The `zmq` crate builds libzmq from source and links it statically.
  MPL-2.0 then requires that we tell users where to get the libzmq source.
  The `.deb` and the AppImage will contain this notice.

## 2. Workspace layout

```text
fasttick/
  Cargo.toml              workspace, edition 2024, shared lint settings
  crates/
    fasttick-core/        data model, lossless XML, node models, validation,
                          layout, edit history, project file
    fasttick-protocol/    client for the BT.CPP publisher protocol
    fasttick-log/         .btlog reader and writer, SQLite reader (optional),
                          replay index, statistics, export
    fasttick-app/         egui/eframe GUI, binary "fasttick"
    fasttick-testkit/     dev-only, not published: builds and starts test_harness
  test_harness/           C++17 CMake executor that links BT.CPP 4.x
  tests/fixtures/         XML from the BT.CPP repository (MIT, with attribution)
  docs/                   plan, design notes, one demo note for each milestone
  deny.toml, about.toml   licence and advisory checks, licence report template
  LICENSE (MIT), THIRD_PARTY_LICENSES.md
```

Dependency rules:

- `fasttick-protocol` and `fasttick-log` use `fasttick-core`. They do not use each other.
- `fasttick-app` uses the three library crates.
- Only `fasttick-app` uses egui. The library crates have no GUI dependencies.
- The GUI crate contains no format logic and no protocol logic.

### 2.1 fasttick-core

| Module | Main types | Purpose |
|---|---|---|
| `xml` | `Document`, `NodeId`, `Node` (Element, Text, Comment, CData, ProcessingInstruction, Declaration, DocType), `Attribute`, `XmlError` | Lossless XML tree, built on the quick-xml event API. It keeps the attribute order, the quote style, the original escapes, the comments and the whitespace. If nothing changed, `write()` gives the same bytes as the input. |
| `bt` | `BtFile`, `TreeRef`, `NodeRef`, `NodeKind`, `PreCondition`, `PostCondition`, `Remapping`, `Include` | BT.CPP v4 view on top of `xml::Document`. It reads `BTCPP_format`, `main_tree_to_execute`, `<BehaviorTree>`, `<SubTree>` with port remapping and `_autoremap`, `<include path ros_pkg>`, `<TreeNodesModel>`, and the condition attributes `_failureIf`, `_successIf`, `_skipIf`, `_while`, `_onSuccess`, `_onFailure`, `_onHalted`, `_post`. It accepts the compact form (`<Sequence>`) and the explicit form (`<Control ID="Sequence">`). |
| `status` | `NodeStatus` (Idle, Running, Success, Failure, Skipped), `RawStatus` | Shared by the protocol and log crates. Decodes the codes 11, 12 and 13 ("idle, previous status was running, success or failure"). |
| `runtime` | `RuntimeTree`, `RuntimeNode`, `SubtreeInstance`, `Uid` | Reads the flat XML that the executor sends and that `.btlog` files contain (`_uid`, `_fullpath`). Gives the node for each UID. |
| `models` | `NodeModel`, `PortModel`, `PortDirection`, `ModelRegistry`, `ModelSource` | Built-in models, imported models and project models. Import of the XML from `writeTreeNodesModelXML`. Export in the same format. |
| `builtin` | embedded `builtin_models_4.9.0.xml`, `ChildRule` | The test harness writes `writeTreeNodesModelXML(factory, true)` from BT.CPP 4.9.0. We embed that output. The child-count rules come from the BT.CPP source. |
| `edit` | `Edit` (invertible operations), `Transaction`, `History` | All changes to a document go through this module. It gives undo and redo. The GUI does not change a document directly. |
| `validate` | `Problem`, `ProblemCode`, `Severity`, `Validator` | All live-validation rules (section 4). |
| `layout` | `LayoutDirection`, `LayoutParams`, `Layout` | Tidy-tree layout (Walker algorithm, linear time). Top-down and left-right. |
| `search` | `Query`, `Match` | Find nodes by name, type or port value. |
| `project` | `Project`, `ProjectFile`, `RecentProjects` | FastTick project file (JSON, extension `.ftproj`). It lists the XML files and model files. It also stores manual node positions, unconnected nodes and editor settings. |

### 2.2 fasttick-protocol

| Module | Main types | Purpose |
|---|---|---|
| `wire` | `RequestType`, `RequestHeader`, `ReplyHeader`, `TreeUuid`, `ProtocolError` | Encode and decode functions only. No sockets. Request header: 6 bytes (protocol = 2, type, id as u32 LE). Reply header: 22 bytes (request header and 16-byte tree UUID). |
| `messages` | `Hook` (`enabled`, `uid`, `mode`, `once`, `desired_status`, `position`), `HookMode`, `HookPosition`, `UnlockRequest`, `RemoveRequest`, `StatusSnapshot`, `RecordedTransition`, `BlackboardDump` | JSON with the exact field names of BT.CPP. MessagePack decode of the blackboard reply. 9-byte transition records. |
| `client` | `Client`, `ClientConfig` | Blocking client for one background thread. REQ socket on `port`. SUB socket on `port + 1`. One method for each of the 12 request types. After a timeout, it closes the REQ socket and opens a new one. |
| `session` | `LiveSession`, `SessionCommand`, `SessionEvent`, `ConnectionState`, `PollConfig` | Background thread. It talks to the GUI through channels. It does the polling, the heartbeat, the automatic reconnect, the restart detection (the tree UUID changes) and the hook cleanup on disconnect. |
| `breakpoints` | `Breakpoint`, `BreakpointSet` | Breakpoint file (JSON, `.ftbp.json`). It stores the node path and the UID, so a set can load again after the tree changes. |

The 12 request types: FULLTREE `T`, STATUS `S`, BLACKBOARD `B`, HOOK_INSERT `I`, HOOK_REMOVE `R`, BREAKPOINT_REACHED `N`, BREAKPOINT_UNLOCK `U`, HOOKS_DUMP `D`, REMOVE_ALL_HOOKS `A`, DISABLE_ALL_HOOKS `X`, TOGGLE_RECORDING `r`, GET_TRANSITIONS `t`.

### 2.3 fasttick-log

| Module | Main types | Purpose |
|---|---|---|
| `btlog` | `BtLog`, `BtLogReader`, `BtLogWriter`, `LogError` | Layout: text `BTCPP4-FileLogger2`, 1 version byte (1), XML length (i32 LE), XML, start time (i64 LE, µs since epoch), then 9-byte records (6-byte time offset, u16 UID, u8 status). The reader accepts a cut-off last record (the executor can still write the file). The writer saves live recordings. |
| `sqlite` (feature `sqlite`) | `SqliteLog` | Reads the tables `Definitions`, `Nodes` and `Transitions`. Optional. |
| `replay` | `ReplayIndex`, `Cursor`, `PlaybackSpeed` | Keeps a full snapshot every N transitions. "State at time t" stays fast on large logs. |
| `stats` | `NodeStats`, `compute_stats` | For each node: time in RUNNING, number of status changes, number of SUCCESS and FAILURE results. Works on a time range. |
| `filter`, `export` | `TransitionFilter`, `export_json`, `export_csv` | Include and exclude filters (node name, status, time range). Export of the filtered rows. |

### 2.4 fasttick-app

| Module | Main types | Purpose |
|---|---|---|
| `app` | `FastTickApp`, `Workspace`, `Mode` (Edit, Live, Replay) | Top-level state. Window title "FastTick". |
| `dock` | `Tab`, `egui_dock::DockState<Tab>` | Tree tabs, split view, side panels. |
| `canvas` | `TreeCanvas`, `Viewport`, `Interaction` | Own drawing code on `egui::Painter`. It draws only the visible nodes and keeps text layouts in a cache. |
| `panels` | `TreeList`, `Palette`, `Inspector`, `ProblemsPanel`, `XmlPanel`, `SearchPanel`, `LivePanel`, `BlackboardPanel`, `BreakpointPanel`, `ReplayPanel`, `TransitionsTable`, `StatsTable` | One type for each panel. |
| `live` | `LiveController` | Holds the channel ends of `LiveSession`. Never blocks the UI thread. |
| `settings`, `shortcuts`, `theme` | `Settings`, `KeyMap`, `Theme` | Recent projects, key map and help dialog, light and dark `Visuals`. |

Node-graph crate: we do not use a dataflow node-graph crate.
These crates model free pins and wires.
FastTick needs ordered parent/child edges, a tidy-tree layout and culling at 1,000+ nodes.
A custom painter gives all three.

### 2.5 test_harness (C++17, CMake)

- `fasttick_executor`: loads an XML file, registers custom nodes and runs a tree.
  It uses the BT.CPP publisher and `FileLogger2`.
  Flags: XML file, tree ID, port, log path, tick period, number of loops, heartbeat timeout.
  It writes `READY` on stdout when the publisher is up.
- Custom nodes: a sync action, an async (stateful) action, a condition, a node with typed ports
  (int, double, string, a JSON-exported struct) and a node that writes blackboard keys.
- `fasttick_dump_models`: writes `writeTreeNodesModelXML(factory, true)`.
  We use it to make the built-in list and the custom-model fixture.
- Sample trees: SubTree with remapping and `_autoremap`, several trees in one file,
  `<include>`, pre/post conditions, and a long loop for the live tests.
- `fasttick-testkit` builds the harness once with CMake, into `target/`.
  If CMake cannot find `behaviortree_cpp`, each integration test writes
  "SKIPPED: BT.CPP 4.x not found (set FASTTICK_BTCPP_PREFIX)" and passes.

### 2.6 Crates that section 3 of the brief does not name

| Crate | Licence | Reason |
|---|---|---|
| `rmpv` | MIT | The BLACKBOARD reply is MessagePack (`nlohmann::json::to_msgpack`), not JSON. We convert it to `serde_json::Value`. |
| `egui_kittest` | MIT OR Apache-2.0 | Headless UI tests of the authoring flows. |
| `criterion` (dev only) | MIT OR Apache-2.0 | Benchmarks for trees with 1,000 and 5,000 nodes. |

## 3. Protocol facts from BT.CPP 4.9.0 that change the design

1. The publisher installs each hook as a **pre-tick** callback. It cannot stop **after** a node ticks.
   It stores a POST hook as a PRE hook. Unlock or remove with position POST then fails with "Node ID not found". See decision D4.
2. HOOKS_DUMP lists only the PRE hooks.
3. Bad JSON in HOOK_INSERT, HOOK_REMOVE or BREAKPOINT_UNLOCK causes an exception in the publisher thread. This can stop the executor.
   FastTick sends only JSON that it makes from typed structs. Tests check each field.
4. To disable a breakpoint that is hit now, HOOK_INSERT with `enabled: false` is not sufficient. The blocked node does not wake up.
   FastTick first sends BREAKPOINT_UNLOCK, then the change.
5. The publisher disables all hooks when it gets no request for 5 s (default).
   FastTick sends a request at least once each second, at all poll rates.
6. The recording buffer keeps only the last 1000 transitions.
   FastTick reads GET_TRANSITIONS every 100 ms during a recording. It shows a warning when a batch is full.
7. An error reply has 2 parts: the text `error` and a message. It has no reply header.
8. "Breakpoint reached" comes on the PUB socket (port + 1). Part 1 is a request header with type `N`. Part 2 is the UID as decimal text.
9. Each reply contains the tree UUID. A new UUID means that the executor restarted. FastTick then gets the tree again.
10. The STATUS reply has 3 bytes for each node: UID (u16 LE) and one status byte.
11. To resume with SUCCESS or FAILURE, send that value in BREAKPOINT_UNLOCK. The node then does not tick. Any other value lets the node tick normally.
12. Fault injection uses HOOK_INSERT with `mode: 1` (replace) and `desired_status` SUCCESS or FAILURE.

## 4. Validation rules

| Code | Rule |
|---|---|
| `UnknownNodeType` | No model has this node type. |
| `MissingRequiredPort` | An input or in/out port has no value and no default. (We confirm the exact rule from `createNodeFromXML` in M1.) |
| `UnknownPort` | The attribute is not a port of the model and not a reserved attribute. |
| `InvalidName` | The name breaks the BT.CPP name rules (`validateModelName`, `validatePortName`, `validateInstanceName`). |
| `ChildCount` | Leaf: 0 children. Decorator: 1. Control: 1 or more. Special rules: `IfThenElse` and `WhileDoElse` 2 or 3, `TryCatch` 2 or more, `SwitchN` N + 1. |
| `DuplicateTreeId` | Two `<BehaviorTree>` elements have the same `ID` in the project. |
| `MissingSubTree` | A `<SubTree>` refers to a tree that does not exist. |
| `SelfSubTree`, `CircularSubTree` | A tree refers to itself, directly or through other trees. |
| `BadRemapping` | A SubTree remaps a port that the target tree does not declare (if the target has a model). |
| `UnconnectedNode` | A node has no parent. It is not written to the XML (see D5). |

Problems show on the node in the canvas and in the problems list.

## 5. Milestones

Each milestone ends with these steps:

1. `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` and `cargo deny check` pass.
2. A short demo note in `docs/demo/Mn.md`.
3. An update of `THIRD_PARTY_LICENSES.md`.
4. A commit and a push. The user asked for a commit and a push every few minutes during the work.

### M1: XML model, round trip and editor

1. Workspace, licence files, `deny.toml`, `about.toml`, CI workflow file, harness skeleton. BT.CPP build for the tests (see D2).
2. Lossless XML module. Round-trip tests with `proptest`. Round-trip tests on all BT.CPP XML: the 19 XML files and the XML text inside the C++ examples and tests (a script extracts it).
3. BT view, node models, embedded built-in list, model import.
4. Edit operations, undo and redo, validation, layout, search core, project file.
5. GUI: tree list, tabs, split view, canvas (add, delete, move, connect, disconnect, reorder, multi-select, copy, paste, duplicate, zoom, pan, fit to view, automatic layout), palette, model editor, inspector, problems list, XML panel (read-only), SubTree actions, save, save as, recent projects.
6. UI tests with `egui_kittest` for the main authoring flows.

### M2: Live debugging

1. `wire` and `messages` modules. Unit tests with fixed byte samples.
2. `Client` and `LiveSession`: polling, heartbeat, reconnect, restart detection, cleanup.
3. Integration tests with the harness: each of the 12 request types, breakpoints, replace mode, recording, disconnect cleanup, executor restart. A fake server sends bad and old messages to the client.
4. GUI: connection bar, live tree with status colours, blackboard viewer, breakpoint list and breakpoint files, markers for substituted nodes, recording to `.btlog`.

### M3: Replay

1. `.btlog` reader and writer. Tests with logs from the harness. Optional SQLite reader.
2. Replay index, statistics, filters, JSON and CSV export.
3. GUI: timeline (play, pause, step, speed, scrub, zoom to a time range), transitions table, statistics table.

### M4: Polish

1. Search panel, themes, shortcut help dialog.
2. Performance: benchmarks and fixes for 1,000 and 5,000 nodes (edit, live, replay).
3. Optional: editable XML panel, v3 to v4 conversion.
4. Packages: `.deb` with `cargo deb`, AppImage. User guide.

## 6. Decisions (approved 2026-10-14)

| ID | Question | Recommendation |
|---|---|---|
| D1 | Project licence | Decided: MIT. |
| D2 | Build environment. This session runs in a cloud container (Ubuntu 24.04), not on your Kubuntu machine. Rust 1.97 is already here, so no rustup step is necessary. ROS, BT.CPP and libzmq are not here. | Install `libzmq3-dev` and `cppzmq-dev` with apt. Build BT.CPP 4.9.0 from the reference clone into `/home/user/ref/btcpp-install` (outside the repository). On your machine, the harness links the ROS Jazzy 4.9.0 package with the same CMake code. |
| D3 | Push. The session pushes to branch `claude/fasttick-ide-build-mcjem9` of `Manos-G/fasttick`. Your brief says "do not push until the user says so". | Push to that branch at the end of each milestone. No pull request unless you ask. The container is temporary, so unpushed work can be lost. |
| D4 | "After" breakpoints. BT.CPP 4.9.0 cannot stop after a node ticks (fact 1). | The UI gives "before" breakpoints. It shows "after" as not available, with the reason. The protocol crate keeps full support for a later BT.CPP version. |
| D5 | Unconnected nodes. BT XML cannot store a node that has no parent. | Keep unconnected nodes in the project file. Show a warning. Do not write them to the XML. |
| D6 | Extra crates (section 2.6) and the dev-only crate `fasttick-testkit`. | Approve. |
