// SPDX-License-Identifier: MIT

#include "nodes.hpp"

#include <behaviortree_cpp/action_node.h>
#include <behaviortree_cpp/condition_node.h>

#include <cmath>
#include <cstdint>
#include <string>
#include <tuple>
#include <vector>

namespace
{

/// Sync action. It reads the input port "message" (default "hello"). It
/// returns SUCCESS, or FAILURE if the port has no value (for example a
/// remapped blackboard entry that nobody wrote).
class SayMessage : public BT::SyncActionNode
{
public:
  SayMessage(const std::string& name, const BT::NodeConfig& config)
    : BT::SyncActionNode(name, config)
  {}

  static BT::PortsList providedPorts()
  {
    return { BT::InputPort<std::string>("message", "hello",
                                        "Text that the node reads on each tick") };
  }

  BT::NodeStatus tick() override
  {
    const auto message = getInput<std::string>("message");
    return message ? BT::NodeStatus::SUCCESS : BT::NodeStatus::FAILURE;
  }
};

/// Async action. It stays RUNNING for the number of ticks in the input port
/// "ticks", then returns SUCCESS. With ticks <= 0 it returns SUCCESS at once.
class WaitTicks : public BT::StatefulActionNode
{
public:
  WaitTicks(const std::string& name, const BT::NodeConfig& config)
    : BT::StatefulActionNode(name, config)
  {}

  static BT::PortsList providedPorts()
  {
    return { BT::InputPort<int>("ticks", 3,
                                "Number of ticks in RUNNING before the node returns "
                                "SUCCESS") };
  }

  BT::NodeStatus onStart() override
  {
    const auto ticks = getInput<int>("ticks");
    if(!ticks)
    {
      throw BT::RuntimeError("WaitTicks: cannot read port [ticks]: ", ticks.error());
    }
    target_ = ticks.value();
    count_ = 0;
    return target_ <= 0 ? BT::NodeStatus::SUCCESS : BT::NodeStatus::RUNNING;
  }

  BT::NodeStatus onRunning() override
  {
    ++count_;
    return count_ >= target_ ? BT::NodeStatus::SUCCESS : BT::NodeStatus::RUNNING;
  }

  void onHalted() override
  {
    count_ = 0;
  }

private:
  int target_ = 0;
  int count_ = 0;
};

/// Condition. It reads a boolean from the blackboard through the input port
/// "flag". It returns SUCCESS if the value is true. It returns FAILURE if the
/// value is false or if the blackboard entry has no value.
class CheckFlag : public BT::ConditionNode
{
public:
  CheckFlag(const std::string& name, const BT::NodeConfig& config)
    : BT::ConditionNode(name, config)
  {}

  static BT::PortsList providedPorts()
  {
    return { BT::InputPort<bool>("flag", "Blackboard entry to check, for example "
                                         "{door_open}") };
  }

  BT::NodeStatus tick() override
  {
    const auto flag = getInput<bool>("flag");
    return (flag && flag.value()) ? BT::NodeStatus::SUCCESS : BT::NodeStatus::FAILURE;
  }
};

/// Sync action. It writes values of several types to the blackboard. The
/// values change on each tick, so a live view can see the updates.
class WriteValues : public BT::SyncActionNode
{
public:
  WriteValues(const std::string& name, const BT::NodeConfig& config)
    : BT::SyncActionNode(name, config)
  {}

  static BT::PortsList providedPorts()
  {
    return {
      BT::OutputPort<int>("int_value", "An integer (42 plus the tick count)"),
      BT::OutputPort<double>("double_value", "A floating-point number"),
      BT::OutputPort<std::string>("string_value", "A text"),
      BT::OutputPort<std::vector<int>>("vector_value", "A vector of integers"),
      BT::OutputPort<Pose2D>("pose", "A Pose2D struct (JSON object)"),
      BT::OutputPort<std::vector<Pose2D>>("poses", "A vector of Pose2D (JSON array)"),
    };
  }

  BT::NodeStatus tick() override
  {
    const int n = count_++;
    const Pose2D pose{ 1.0 + n, 2.0, 0.5 };
    // Ports that the XML does not remap give an error. That is not a
    // problem for this node, so the results are not checked.
    std::ignore = setOutput("int_value", 42 + n);
    std::ignore = setOutput("double_value", 3.25 * (n + 1));
    std::ignore = setOutput("string_value", std::string("tick ") + std::to_string(n));
    std::ignore = setOutput("vector_value", std::vector<int>{ 1, 2, 3, n });
    std::ignore = setOutput("pose", pose);
    std::ignore = setOutput("poses", std::vector<Pose2D>{ pose, Pose2D{ 0.0, 0.0, 0.0 } });
    return BT::NodeStatus::SUCCESS;
  }

private:
  int count_ = 0;
};

/// Sync action with an in/out port. It reads "counter" (no value counts as
/// 0), adds "step" and writes the result back to "counter".
class IncrementCounter : public BT::SyncActionNode
{
public:
  IncrementCounter(const std::string& name, const BT::NodeConfig& config)
    : BT::SyncActionNode(name, config)
  {}

  static BT::PortsList providedPorts()
  {
    return {
      BT::BidirectionalPort<int>("counter", "Value to increment. The node writes "
                                            "the new value back"),
      BT::InputPort<int>("step", 1, "Amount to add on each tick"),
    };
  }

  BT::NodeStatus tick() override
  {
    const auto step = getInput<int>("step");
    if(!step)
    {
      throw BT::RuntimeError("IncrementCounter: cannot read port [step]: ",
                             step.error());
    }
    const auto counter = getInput<int>("counter");
    const int value = counter ? counter.value() : 0;
    const auto written = setOutput("counter", value + step.value());
    if(!written)
    {
      throw BT::RuntimeError("IncrementCounter: cannot write port [counter]: ",
                             written.error());
    }
    return BT::NodeStatus::SUCCESS;
  }
};

/// Sync action that reads a Pose2D. It returns FAILURE if the port has no
/// valid value. It writes the distance from the origin to "distance".
class ReadPose : public BT::SyncActionNode
{
public:
  ReadPose(const std::string& name, const BT::NodeConfig& config)
    : BT::SyncActionNode(name, config)
  {}

  static BT::PortsList providedPorts()
  {
    return {
      BT::InputPort<Pose2D>("pose", "Pose to read. Text form: \"x;y;theta\""),
      BT::OutputPort<double>("distance", "Distance of the pose from the origin"),
    };
  }

  BT::NodeStatus tick() override
  {
    const auto pose = getInput<Pose2D>("pose");
    if(!pose)
    {
      return BT::NodeStatus::FAILURE;
    }
    std::ignore = setOutput("distance", std::hypot(pose->x, pose->y));
    return BT::NodeStatus::SUCCESS;
  }
};

/// Sync action with metadata. BT.CPP 4.9.0 reads the static metadata()
/// function (see CreateManifest in bt_factory.h) and writes it to the
/// model XML as <MetadataFields>.
class AnnotatedAction : public BT::SyncActionNode
{
public:
  AnnotatedAction(const std::string& name, const BT::NodeConfig& config)
    : BT::SyncActionNode(name, config)
  {}

  static BT::PortsList providedPorts()
  {
    return { BT::InputPort<double>("weight", 1.5, "A double port with a default "
                                                  "value") };
  }

  static BT::KeyValueVector metadata()
  {
    return { { "description", "Action that carries metadata for the model tests" },
             { "category", "fasttick-test" } };
  }

  BT::NodeStatus tick() override
  {
    const auto weight = getInput<double>("weight");
    return weight ? BT::NodeStatus::SUCCESS : BT::NodeStatus::FAILURE;
  }
};

}  // namespace

void registerFastTickTestNodes(BT::BehaviorTreeFactory& factory)
{
  factory.registerNodeType<SayMessage>("SayMessage");
  factory.registerNodeType<WaitTicks>("WaitTicks");
  factory.registerNodeType<CheckFlag>("CheckFlag");
  factory.registerNodeType<WriteValues>("WriteValues");
  factory.registerNodeType<IncrementCounter>("IncrementCounter");
  factory.registerNodeType<ReadPose>("ReadPose");
  factory.registerNodeType<AnnotatedAction>("AnnotatedAction");

  // JsonExporter keeps a global map. A second call adds nothing new.
  BT::RegisterJsonDefinition<Pose2D>();
}
