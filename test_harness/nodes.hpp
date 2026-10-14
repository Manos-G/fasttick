// SPDX-License-Identifier: MIT
//
// Custom tree nodes for the FastTick test executor.
//
// The nodes cover the node kinds and port kinds that FastTick must show:
// a sync action with a default input value, an async (stateful) action,
// a condition, an action that writes blackboard values of several types
// (including a JSON-exported struct), an action with an in/out port and
// an action with metadata.

#pragma once

#include <behaviortree_cpp/bt_factory.h>
#include <behaviortree_cpp/json_export.h>

#include <string>

/// A 2D pose. The JSON form is {"x": .., "y": .., "theta": .., "__type": "Pose2D"}.
struct Pose2D
{
  double x = 0.0;
  double y = 0.0;
  double theta = 0.0;
};

// BT_JSON_CONVERTER comes from behaviortree_cpp/json_export.h. It defines
// to_json() and from_json() for nlohmann::json. The blackboard dump of the
// publisher then shows a Pose2D as a nested JSON object.
// NOLINTNEXTLINE(misc-use-internal-linkage)
BT_JSON_CONVERTER(Pose2D, pose)
{
  add_field("x", &pose.x);
  add_field("y", &pose.y);
  add_field("theta", &pose.theta);
}

namespace BT
{
/// Converts the text "x;y;theta" (or "json:{...}") to a Pose2D.
///
/// BT.CPP calls this function when an input port of type Pose2D gets a
/// text value. The pattern is the same as in sample_nodes/movebase_node.h.
template <>
inline Pose2D convertFromString<Pose2D>(StringView str)
{
  if(StartWith(str, "json:"))
  {
    str.remove_prefix(5);
    return convertFromJSON<Pose2D>(str);
  }
  const auto parts = splitString(str, ';');
  if(parts.size() != 3)
  {
    throw RuntimeError("Pose2D: expected the text \"x;y;theta\", got \"",
                       std::string(str), "\"");
  }
  Pose2D pose;
  pose.x = convertFromString<double>(parts[0]);
  pose.y = convertFromString<double>(parts[1]);
  pose.theta = convertFromString<double>(parts[2]);
  return pose;
}
}  // namespace BT

/// Registers all FastTick test nodes in the factory.
///
/// It also registers the JSON converter of Pose2D, so that the publisher
/// can export Pose2D values (and vectors of Pose2D) from the blackboard.
void registerFastTickTestNodes(BT::BehaviorTreeFactory& factory);
