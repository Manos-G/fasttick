# FastTick

FastTick is a fast tree editor and debugger for
[BehaviorTree.CPP](https://github.com/BehaviorTree/BehaviorTree.CPP) v4.
It is a desktop application. Use it to write behavior trees, to debug a running
executor, and to replay log files. All features are free.

Status: in development. See [docs/PLAN.md](docs/PLAN.md).

## Features (planned)

- Edit BT.CPP v4 XML files (`BTCPP_format="4"`) without loss of data.
- Import custom node models from `BT::writeTreeNodesModelXML`.
- Connect to an executor that runs the BT.CPP publisher: live status,
  blackboards, breakpoints, fault injection and recording.
- Open `.btlog` files from `BT::FileLogger2` and replay them.

## Build

1. Install Rust (stable) with rustup.
2. Run `cargo build --release`.
3. The binary is `target/release/fasttick`.

The integration tests need BT.CPP 4.x. Set `FASTTICK_BTCPP_PREFIX` to the
install prefix of BT.CPP (for ROS 2 Jazzy: `/opt/ros/jazzy`). If the tests
cannot find BT.CPP, they skip.

## Licence

MIT. See [LICENSE](LICENSE). Third-party licences are in
[THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md).
