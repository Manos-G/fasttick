// SPDX-License-Identifier: MIT
//
// fasttick_dump_models: prints the <TreeNodesModel> XML of the FastTick test
// nodes (BT::writeTreeNodesModelXML). With --include-builtin, the output
// also contains the built-in nodes of BT.CPP.

#include "nodes.hpp"

#include <behaviortree_cpp/bt_factory.h>
#include <behaviortree_cpp/xml_parsing.h>

#include <cstring>
#include <exception>
#include <iostream>
#include <string>

int main(int argc, char** argv)
{
  bool include_builtin = false;
  for(int i = 1; i < argc; ++i)
  {
    if(std::strcmp(argv[i], "--include-builtin") == 0)
    {
      include_builtin = true;
    }
    else if(std::strcmp(argv[i], "-h") == 0 || std::strcmp(argv[i], "--help") == 0)
    {
      std::cout << "Usage: fasttick_dump_models [--include-builtin]\n"
                   "\n"
                   "Prints the TreeNodesModel XML of the FastTick test nodes.\n"
                   "  --include-builtin   Also print the built-in BT.CPP nodes.\n";
      return 0;
    }
    else
    {
      std::cerr << "fasttick_dump_models: unknown option: \"" << argv[i] << "\"\n";
      return 2;
    }
  }

  try
  {
    BT::BehaviorTreeFactory factory;
    registerFastTickTestNodes(factory);
    std::cout << BT::writeTreeNodesModelXML(factory, include_builtin) << std::endl;
  }
  catch(const std::exception& error)
  {
    std::cerr << "fasttick_dump_models: error: " << error.what() << std::endl;
    return 1;
  }
  return 0;
}
