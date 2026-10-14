# SPDX-License-Identifier: MIT
#
# FastTick test harness: a small executor that links BehaviorTree.CPP 4.x.
#
# Configure with the BT.CPP install prefix, for example:
#   cmake -S test_harness -B build -DCMAKE_PREFIX_PATH=/opt/ros/jazzy
#   cmake --build build

cmake_minimum_required(VERSION 3.16)
project(fasttick_test_harness LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

if(NOT CMAKE_BUILD_TYPE AND NOT CMAKE_CONFIGURATION_TYPES)
  set(CMAKE_BUILD_TYPE RelWithDebInfo CACHE STRING "Build type" FORCE)
endif()

option(FASTTICK_WERROR "Treat compiler warnings as errors" OFF)

find_package(Threads REQUIRED)
find_package(behaviortree_cpp REQUIRED)

# The package name "behaviortree_cpp" is BT.CPP 4.x (BT.CPP 3.x uses
# "behaviortree_cpp_v3"). If a version file exists, check it too.
if(DEFINED behaviortree_cpp_VERSION AND NOT "${behaviortree_cpp_VERSION}" STREQUAL "")
  if(behaviortree_cpp_VERSION VERSION_LESS 4.0)
    message(FATAL_ERROR "BT.CPP 4.x is necessary, found ${behaviortree_cpp_VERSION}")
  endif()
  message(STATUS "FastTick harness: BT.CPP version ${behaviortree_cpp_VERSION}")
endif()

# A plain CMake install of BT.CPP exports BT::behaviortree_cpp
# (cmake/conan_build.cmake). The ROS 2 (ament) package exports
# behaviortree_cpp::behaviortree_cpp (cmake/ament_build.cmake). Very old
# packages give only variables.
if(TARGET BT::behaviortree_cpp)
  set(FASTTICK_BTCPP BT::behaviortree_cpp)
elseif(TARGET behaviortree_cpp::behaviortree_cpp)
  set(FASTTICK_BTCPP behaviortree_cpp::behaviortree_cpp)
else()
  add_library(fasttick_btcpp INTERFACE)
  target_include_directories(fasttick_btcpp SYSTEM INTERFACE ${behaviortree_cpp_INCLUDE_DIRS})
  target_link_libraries(fasttick_btcpp INTERFACE ${behaviortree_cpp_LIBRARIES})
  set(FASTTICK_BTCPP fasttick_btcpp)
endif()
message(STATUS "FastTick harness: BT.CPP target ${FASTTICK_BTCPP}")

# Check if this BT.CPP build has the SqliteLogger (option BTCPP_SQLITE_LOGGING).
include(CheckCXXSourceCompiles)
if(TARGET ${FASTTICK_BTCPP} AND NOT FASTTICK_BTCPP STREQUAL "fasttick_btcpp")
  set(CMAKE_REQUIRED_LIBRARIES ${FASTTICK_BTCPP})
else()
  set(CMAKE_REQUIRED_INCLUDES ${behaviortree_cpp_INCLUDE_DIRS})
  set(CMAKE_REQUIRED_LIBRARIES ${behaviortree_cpp_LIBRARIES})
endif()
check_cxx_source_compiles("
#include <behaviortree_cpp/bt_factory.h>
#include <behaviortree_cpp/loggers/bt_sqlite_logger.h>
int main()
{
  BT::Tree tree;
  BT::SqliteLogger logger(tree, \"check.db3\");
  return 0;
}" FASTTICK_HAVE_SQLITE_LOGGER)
unset(CMAKE_REQUIRED_INCLUDES)
unset(CMAKE_REQUIRED_LIBRARIES)

# libzmq (C API) is optional. With it, the executor can release a blocked
# breakpoint when it gets SIGINT or SIGTERM.
find_path(FASTTICK_ZMQ_INCLUDE_DIR zmq.h)
find_library(FASTTICK_ZMQ_LIBRARY NAMES zmq libzmq)

set(FASTTICK_WARNINGS "")
if(CMAKE_CXX_COMPILER_ID MATCHES "GNU|Clang")
  set(FASTTICK_WARNINGS -Wall -Wextra -Wpedantic)
  if(FASTTICK_WERROR)
    list(APPEND FASTTICK_WARNINGS -Werror)
  endif()
endif()

add_library(fasttick_test_nodes STATIC nodes.cpp)
target_include_directories(fasttick_test_nodes PUBLIC ${CMAKE_CURRENT_SOURCE_DIR})
target_link_libraries(fasttick_test_nodes PUBLIC ${FASTTICK_BTCPP} Threads::Threads)
target_compile_options(fasttick_test_nodes PRIVATE ${FASTTICK_WARNINGS})

add_executable(fasttick_executor executor.cpp)
target_link_libraries(fasttick_executor PRIVATE fasttick_test_nodes)
target_compile_options(fasttick_executor PRIVATE ${FASTTICK_WARNINGS})
if(FASTTICK_HAVE_SQLITE_LOGGER)
  target_compile_definitions(fasttick_executor PRIVATE FASTTICK_HAVE_SQLITE_LOGGER=1)
else()
  message(STATUS "FastTick harness: no SqliteLogger in BT.CPP; --sqlite is not available")
endif()
if(FASTTICK_ZMQ_INCLUDE_DIR AND FASTTICK_ZMQ_LIBRARY)
  target_include_directories(fasttick_executor SYSTEM PRIVATE ${FASTTICK_ZMQ_INCLUDE_DIR})
  target_link_libraries(fasttick_executor PRIVATE ${FASTTICK_ZMQ_LIBRARY})
  target_compile_definitions(fasttick_executor PRIVATE FASTTICK_HAVE_ZMQ=1)
  message(STATUS "FastTick harness: libzmq found (${FASTTICK_ZMQ_LIBRARY})")
else()
  message(STATUS "FastTick harness: libzmq not found; a stop signal cannot release a blocked breakpoint")
endif()

add_executable(fasttick_dump_models dump_models.cpp)
target_link_libraries(fasttick_dump_models PRIVATE fasttick_test_nodes)
target_compile_options(fasttick_dump_models PRIVATE ${FASTTICK_WARNINGS})
