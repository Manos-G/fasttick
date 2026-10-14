# FastTick test harness

This directory contains a small C++17 program that runs behavior trees with
BehaviorTree.CPP (BT.CPP) 4.x. The FastTick tests use it as a real executor.

It contains:

- `fasttick_executor`: loads an XML file, registers the test nodes and ticks
  one tree. It can start the BT.CPP publisher, a
  `FileLogger2` (`.btlog`) and a `SqliteLogger` (`.db3`).
- `fasttick_dump_models`: prints the `<TreeNodesModel>` XML of the test nodes.
- `trees/`: sample trees for the tests.

## Requirements

- CMake 3.16 or later and a C++17 compiler.
- BT.CPP 4.x with its CMake package files. For example the ROS 2 Jazzy
  package (`/opt/ros/jazzy`) or a local install.
- Optional: `libzmq3-dev`. With it, a stop signal can release a node that
  waits at a breakpoint.

## Build

```sh
cmake -S test_harness -B test_harness/build -DCMAKE_PREFIX_PATH=/opt/ros/jazzy
cmake --build test_harness/build
```

Replace `/opt/ros/jazzy` with your BT.CPP install prefix.

The Rust tests build the harness automatically (see the crate
`crates/fasttick-testkit`). They look for BT.CPP in this order: the
environment variable `FASTTICK_BTCPP_PREFIX`, `/opt/ros/jazzy`, `/usr/local`
and `/usr`. If they do not find BT.CPP, they write `SKIPPED` and pass.

```sh
FASTTICK_BTCPP_PREFIX=/opt/ros/jazzy cargo test -p fasttick-testkit
```

Set `FASTTICK_REQUIRE_HARNESS=1` to make the tests fail (not skip) when
the harness is not available, for example in a CI job that installs BT.CPP.

## Run the executor

```sh
fasttick_executor --xml test_harness/trees/main.xml --loops 1 --btlog /tmp/main.btlog
```

Options:

| Option | Default | Meaning |
|---|---|---|
| `--xml <file>` | (required) | The XML file. `<include>` is allowed. |
| `--tree <ID>` | see text | The tree to run. Without it: `main_tree_to_execute`, else the first `<BehaviorTree>` in the file. If the file has only `<include>` elements, the first tree ID in alphabetical order. |
| `--port <n>` | 1667 | Publisher port. The publisher also uses `<n> + 1`. `0` means no publisher. |
| `--btlog <path>` | none | Write a `FileLogger2` log. The extension must be `.btlog`. |
| `--sqlite <path>` | none | Write a `SqliteLogger` log. The extension must be `.db3` or `.btdb`. |
| `--tick-ms <n>` | 20 | Sleep time between ticks, in milliseconds. |
| `--loops <n>` | 0 | Run the tree to completion `n` times. The tree is reset between runs. `0` means no limit. |
| `--duration-ms <n>` | 0 | Stop after `n` milliseconds. `0` means no limit. |
| `--heartbeat-ms <n>` | BT.CPP default | Publisher heartbeat timeout (`setMaxHeartbeatDelay`). |
| `--exit-with-parent` | off | Stop cleanly when the parent process ends. The Rust tests use it, so that a killed test process does not leave an executor behind. |

Each option with a value also accepts the form `--option=value`.

Output on stdout, one line for each event:

- `READY port=<n>`: the tree, the publisher and the loggers are ready.
- `RESULT loop=<n> status=<STATUS>`: the root node completed a run.
- `DONE`: normal end (all loops done, or the time limit).
- `STOPPED signal=<n>`: end after SIGINT or SIGTERM.
- `STOPPED parent=exited`: end because the parent process ended (only with
  `--exit-with-parent`).

SIGINT and SIGTERM stop the ticks, halt the running nodes, close the loggers
and then the publisher. The exit code is then 0. A second signal ends the
program at once. If a tick waits at a breakpoint, the executor tells its own
publisher to disable all hooks (this needs libzmq), so the tick can end.
Errors go to stderr. The exit code is 1 for a run-time error and 2 for a bad
command line.

Between two runs (`--loops`), the executor does not call
`BT::Tree::haltTree()`. That function runs the `_onHalted` script of every
node, also of nodes that did not run. A real application that ticks a tree
again does not do that.

## Print the node models

```sh
fasttick_dump_models                    # test nodes only
fasttick_dump_models --include-builtin  # test nodes and built-in nodes
```

## Test nodes

| ID | Kind | Ports |
|---|---|---|
| `SayMessage` | sync action | input `message` (default `hello`) |
| `WaitTicks` | async action (`StatefulActionNode`) | input `ticks` (default 3): stays RUNNING for this number of ticks |
| `CheckFlag` | condition | input `flag` (bool from the blackboard) |
| `WriteValues` | sync action | outputs of type int, double, string, `vector<int>`, `Pose2D` and `vector<Pose2D>` |
| `IncrementCounter` | sync action | in/out `counter`, input `step` (default 1) |
| `ReadPose` | sync action | input `pose` (`Pose2D`, text `x;y;theta`), output `distance` |
| `AnnotatedAction` | sync action with metadata | input `weight` (default 1.5) |

`Pose2D` has a JSON converter, so the publisher shows it as a JSON object
in the blackboard.

## Sample trees

| File | Content |
|---|---|
| `trees/main.xml` | Control nodes, decorators, SubTree with port remapping and with `_autoremap`, all pre- and post-conditions, several trees, `<TreeNodesModel>`, blackboard values of all types. |
| `trees/include_main.xml` | Uses `<include path="included/sub.xml"/>`. |
| `trees/included/sub.xml` | The tree that `include_main.xml` includes. |
| `trees/long_running.xml` | Stays RUNNING until the executor stops. For live and breakpoint tests. |

The samples in `tests/fixtures/harness/` come from this harness. To make
them again, run the smoke tests with `FASTTICK_UPDATE_FIXTURES=1`.

## Licence

MIT, the same as FastTick.
