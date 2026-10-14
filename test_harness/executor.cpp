// SPDX-License-Identifier: MIT
//
// fasttick_executor: loads a BT.CPP 4.x XML file, registers the FastTick
// test nodes and ticks one tree. It can start the BT.CPP publisher, a
// FileLogger2 (.btlog) and a SqliteLogger (.db3).
//
// Output protocol on stdout (one line each, flushed):
//   READY port=<port>                 after the tree, the publisher and the
//                                     loggers are up (port 0: no publisher)
//   RESULT loop=<n> status=<STATUS>   each time the root node completes
//   DONE                              normal end (all loops or the duration)
//   STOPPED signal=<n>                end after SIGINT or SIGTERM
//   STOPPED parent=exited             end because the parent process ended
//                                     (only with --exit-with-parent)
// Errors go to stderr. Exit code 0 for DONE and STOPPED, 1 for a runtime
// error, 2 for a usage error.

#include "nodes.hpp"

#include <behaviortree_cpp/bt_factory.h>
#include <behaviortree_cpp/loggers/bt_file_logger_v2.h>
#include <behaviortree_cpp/loggers/groot2_publisher.h>
#include <behaviortree_cpp/xml_parsing.h>

#ifdef FASTTICK_HAVE_SQLITE_LOGGER
#include <behaviortree_cpp/loggers/bt_sqlite_logger.h>
#endif

#ifdef FASTTICK_HAVE_ZMQ
#include <behaviortree_cpp/loggers/groot2_protocol.h>
#include <zmq.h>
#endif

#include <signal.h>
#include <unistd.h>

#include <algorithm>
#include <atomic>
#include <chrono>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <iterator>
#include <limits>
#include <memory>
#include <optional>
#include <sstream>
#include <string>
#include <thread>

namespace
{

constexpr int kExitOk = 0;
constexpr int kExitRuntimeError = 1;
constexpr int kExitUsage = 2;

// ---------------------------------------------------------------------------
// Signal handling
// ---------------------------------------------------------------------------

static_assert(std::atomic<int>::is_always_lock_free,
              "the signal handler needs a lock-free atomic int");

std::atomic<int> g_stop_signal{ 0 };
std::atomic<int> g_signal_count{ 0 };
// Set by the watcher thread when the parent process ended
// (--exit-with-parent).
std::atomic<bool> g_parent_exited{ false };

extern "C" void onStopSignal(int sig)
{
  // A second signal ends the process at once. This is a way out if a tick
  // cannot finish (for example a breakpoint that nobody unlocks).
  if(g_signal_count.fetch_add(1) >= 1)
  {
    std::_Exit(128 + sig);
  }
  g_stop_signal.store(sig);
}

bool stopRequested()
{
  return g_stop_signal.load() != 0 || g_parent_exited.load();
}

void installSignalHandlers()
{
  struct sigaction action = {};
  action.sa_handler = onStopSignal;
  sigemptyset(&action.sa_mask);
  action.sa_flags = SA_RESTART;
  sigaction(SIGINT, &action, nullptr);
  sigaction(SIGTERM, &action, nullptr);

  // A closed stdout must not kill the process.
  struct sigaction ignore = {};
  ignore.sa_handler = SIG_IGN;
  sigemptyset(&ignore.sa_mask);
  sigaction(SIGPIPE, &ignore, nullptr);
}

// ---------------------------------------------------------------------------
// Command line
// ---------------------------------------------------------------------------

struct Options
{
  std::string xml;
  std::string tree;
  uint64_t port = 1667;
  std::string btlog;
  std::string sqlite;
  uint64_t tick_ms = 20;
  uint64_t loops = 0;
  uint64_t duration_ms = 0;
  std::optional<uint64_t> heartbeat_ms;
  bool exit_with_parent = false;
  bool help = false;
};

class UsageError : public std::runtime_error
{
public:
  using std::runtime_error::runtime_error;
};

void printUsage(std::ostream& out)
{
  out << "Usage: fasttick_executor --xml <file> [options]\n"
         "\n"
         "Loads a BT.CPP 4.x XML file and ticks one tree.\n"
         "\n"
         "Options:\n"
         "  --xml <file>          XML file to load (required). <include> is allowed.\n"
         "  --tree <ID>           Tree to execute. Default: main_tree_to_execute,\n"
         "                        else the first <BehaviorTree> in the file.\n"
         "  --port <n>            Publisher port (default 1667). The publisher also\n"
         "                        uses <n>+1. 0 = no publisher.\n"
         "  --btlog <path>        Write a FileLogger2 log (extension .btlog).\n"
         "  --sqlite <path>       Write a SqliteLogger log (extension .db3 or .btdb).\n"
         "  --tick-ms <n>         Sleep between ticks in ms (default 20).\n"
         "  --loops <n>           Run the tree to completion n times (default 0 =\n"
         "                        forever). The tree is reset between runs.\n"
         "  --duration-ms <n>     Stop after n ms (default 0 = no limit).\n"
         "  --heartbeat-ms <n>    Publisher heartbeat timeout in ms.\n"
         "  --exit-with-parent    Stop cleanly when the parent process ends.\n"
         "  -h, --help            Show this text.\n"
         "\n"
         "stdout: \"READY port=<n>\", \"RESULT loop=<n> status=<S>\", \"DONE\",\n"
         "\"STOPPED signal=<n>\", \"STOPPED parent=exited\".\n"
         "SIGINT and SIGTERM stop the executor cleanly.\n";
}

/// Parses a decimal number without sign. Returns nothing for an empty text,
/// a non-digit character, or a value above max_value.
std::optional<uint64_t> parseUnsigned(const std::string& text, uint64_t max_value)
{
  if(text.empty() || text.size() > 20)
  {
    return std::nullopt;
  }
  uint64_t value = 0;
  for(const char c : text)
  {
    if(c < '0' || c > '9')
    {
      return std::nullopt;
    }
    const auto digit = static_cast<uint64_t>(c - '0');
    if(value > (std::numeric_limits<uint64_t>::max() - digit) / 10)
    {
      return std::nullopt;
    }
    value = value * 10 + digit;
  }
  if(value > max_value)
  {
    return std::nullopt;
  }
  return value;
}

uint64_t requireUnsigned(const std::string& flag, const std::string& text,
                         uint64_t max_value)
{
  const auto value = parseUnsigned(text, max_value);
  if(!value)
  {
    throw UsageError("invalid value for " + flag + ": \"" + text +
                     "\" (expected an integer from 0 to " + std::to_string(max_value) +
                     ")");
  }
  return *value;
}

Options parseOptions(int argc, char** argv)
{
  Options options;
  bool have_xml = false;
  // One day in ms is the limit for times. It keeps the chrono values small.
  constexpr uint64_t kMaxMs = 24ULL * 3600ULL * 1000ULL;

  for(int i = 1; i < argc; ++i)
  {
    std::string arg = argv[i];
    if(arg == "-h" || arg == "--help")
    {
      options.help = true;
      return options;
    }
    if(arg.rfind("--", 0) != 0)
    {
      throw UsageError("unexpected argument: \"" + arg + "\"");
    }

    // Accept "--flag value" and "--flag=value".
    std::string value;
    bool have_value = false;
    const auto eq = arg.find('=');
    if(eq != std::string::npos)
    {
      value = arg.substr(eq + 1);
      arg = arg.substr(0, eq);
      have_value = true;
    }
    auto takeValue = [&]() -> std::string {
      if(have_value)
      {
        return value;
      }
      if(i + 1 >= argc)
      {
        throw UsageError("missing value for " + arg);
      }
      return std::string(argv[++i]);
    };

    if(arg == "--xml")
    {
      options.xml = takeValue();
      have_xml = true;
    }
    else if(arg == "--tree")
    {
      options.tree = takeValue();
      if(options.tree.empty())
      {
        throw UsageError("the value of --tree must not be empty");
      }
    }
    else if(arg == "--port")
    {
      // The publisher binds port and port + 1.
      options.port = requireUnsigned(arg, takeValue(), 65534);
    }
    else if(arg == "--btlog")
    {
      options.btlog = takeValue();
    }
    else if(arg == "--sqlite")
    {
      options.sqlite = takeValue();
    }
    else if(arg == "--tick-ms")
    {
      options.tick_ms = requireUnsigned(arg, takeValue(), kMaxMs);
    }
    else if(arg == "--loops")
    {
      options.loops = requireUnsigned(arg, takeValue(), std::numeric_limits<uint32_t>::max());
    }
    else if(arg == "--duration-ms")
    {
      options.duration_ms = requireUnsigned(arg, takeValue(), kMaxMs);
    }
    else if(arg == "--heartbeat-ms")
    {
      options.heartbeat_ms = requireUnsigned(arg, takeValue(), kMaxMs);
    }
    else if(arg == "--exit-with-parent")
    {
      if(have_value)
      {
        throw UsageError("the option --exit-with-parent takes no value");
      }
      options.exit_with_parent = true;
    }
    else
    {
      throw UsageError("unknown option: \"" + arg + "\"");
    }
  }

  if(!have_xml || options.xml.empty())
  {
    throw UsageError("the option --xml <file> is required");
  }
  return options;
}

// ---------------------------------------------------------------------------
// Default tree ID
// ---------------------------------------------------------------------------

/// Appends the UTF-8 form of a Unicode code point.
void appendUtf8(std::string& out, uint32_t code)
{
  if(code < 0x80)
  {
    out.push_back(static_cast<char>(code));
  }
  else if(code < 0x800)
  {
    out.push_back(static_cast<char>(0xC0 | (code >> 6)));
    out.push_back(static_cast<char>(0x80 | (code & 0x3F)));
  }
  else if(code < 0x10000)
  {
    out.push_back(static_cast<char>(0xE0 | (code >> 12)));
    out.push_back(static_cast<char>(0x80 | ((code >> 6) & 0x3F)));
    out.push_back(static_cast<char>(0x80 | (code & 0x3F)));
  }
  else
  {
    out.push_back(static_cast<char>(0xF0 | (code >> 18)));
    out.push_back(static_cast<char>(0x80 | ((code >> 12) & 0x3F)));
    out.push_back(static_cast<char>(0x80 | ((code >> 6) & 0x3F)));
    out.push_back(static_cast<char>(0x80 | (code & 0x3F)));
  }
}

/// Decodes a character reference ("&#101;" or "&#x65;") at position pos.
/// Returns the length of the reference, or 0 if the text there is not a
/// valid character reference.
size_t decodeCharacterReference(const std::string& text, size_t pos, std::string& out)
{
  if(text.compare(pos, 2, "&#") != 0)
  {
    return 0;
  }
  size_t i = pos + 2;
  // XML 1.0 (CharRef) allows only a lower-case "x" for the hex form.
  const bool hex = i < text.size() && text[i] == 'x';
  if(hex)
  {
    ++i;
  }
  const size_t digits_start = i;
  uint32_t code = 0;
  while(i < text.size() && text[i] != ';')
  {
    const char c = text[i];
    uint32_t digit = 0;
    if(c >= '0' && c <= '9')
    {
      digit = static_cast<uint32_t>(c - '0');
    }
    else if(hex && c >= 'a' && c <= 'f')
    {
      digit = static_cast<uint32_t>(c - 'a' + 10);
    }
    else if(hex && c >= 'A' && c <= 'F')
    {
      digit = static_cast<uint32_t>(c - 'A' + 10);
    }
    else
    {
      return 0;
    }
    code = code * (hex ? 16 : 10) + digit;
    if(code > 0x10FFFF)
    {
      return 0;
    }
    ++i;
  }
  if(i >= text.size() || i == digits_start || code == 0 ||
     (code >= 0xD800 && code <= 0xDFFF))
  {
    return 0;
  }
  appendUtf8(out, code);
  return i + 1 - pos;
}

/// Replaces the five predefined XML entities and the character references.
/// Other entities stay as they are.
std::string decodeXmlEntities(const std::string& text)
{
  static const std::pair<const char*, char> kEntities[] = {
    { "&lt;", '<' }, { "&gt;", '>' }, { "&amp;", '&' }, { "&quot;", '"' }, { "&apos;", '\'' }
  };
  std::string out;
  out.reserve(text.size());
  for(size_t i = 0; i < text.size();)
  {
    bool replaced = false;
    if(text[i] == '&')
    {
      if(const size_t length = decodeCharacterReference(text, i, out); length > 0)
      {
        i += length;
        continue;
      }
      for(const auto& [entity, character] : kEntities)
      {
        const size_t len = std::strlen(entity);
        if(text.compare(i, len, entity) == 0)
        {
          out.push_back(character);
          i += len;
          replaced = true;
          break;
        }
      }
    }
    if(!replaced)
    {
      out.push_back(text[i]);
      ++i;
    }
  }
  return out;
}

struct TopLevelInfo
{
  std::string main_tree_to_execute;
  std::string first_tree_id;
};

/// Scans the XML text for the root attribute main_tree_to_execute and the
/// ID of the first <BehaviorTree> child of the root, in document order.
///
/// BT.CPP keeps its registered trees in a sorted map (XMLParser::PImpl::
/// tree_roots in src/xml_parsing.cpp), so the document order is not
/// available from the factory. This small scanner skips comments, CDATA,
/// processing instructions and DOCTYPE. A malformed file gives an empty
/// result; BT.CPP reports the real error when it loads the file.
TopLevelInfo scanTopLevel(const std::string& text)
{
  TopLevelInfo info;
  int depth = 0;
  size_t pos = 0;
  const size_t n = text.size();

  auto skipPast = [&](const char* terminator) -> bool {
    const auto end = text.find(terminator, pos);
    if(end == std::string::npos)
    {
      return false;
    }
    pos = end + std::strlen(terminator);
    return true;
  };
  auto isSpace = [](char c) { return c == ' ' || c == '\t' || c == '\r' || c == '\n'; };

  while(pos < n)
  {
    const auto lt = text.find('<', pos);
    if(lt == std::string::npos)
    {
      break;
    }
    pos = lt;
    if(text.compare(pos, 4, "<!--") == 0)
    {
      if(!skipPast("-->"))
      {
        break;
      }
      continue;
    }
    if(text.compare(pos, 9, "<![CDATA[") == 0)
    {
      if(!skipPast("]]>"))
      {
        break;
      }
      continue;
    }
    if(text.compare(pos, 2, "<?") == 0)
    {
      if(!skipPast("?>"))
      {
        break;
      }
      continue;
    }
    if(text.compare(pos, 2, "<!") == 0)
    {
      // DOCTYPE, possibly with an internal subset in [ ].
      int brackets = 0;
      ++pos;
      while(pos < n && !(text[pos] == '>' && brackets == 0))
      {
        if(text[pos] == '[')
        {
          ++brackets;
        }
        else if(text[pos] == ']')
        {
          --brackets;
        }
        ++pos;
      }
      ++pos;
      continue;
    }
    if(text.compare(pos, 2, "</") == 0)
    {
      --depth;
      if(depth < 0 || !skipPast(">"))
      {
        break;
      }
      continue;
    }

    // Start tag: name, attributes, then ">" or "/>".
    ++pos;
    const size_t name_start = pos;
    while(pos < n && !isSpace(text[pos]) && text[pos] != '>' && text[pos] != '/')
    {
      ++pos;
    }
    const std::string name = text.substr(name_start, pos - name_start);
    bool self_closing = false;
    bool closed = false;
    while(pos < n)
    {
      while(pos < n && isSpace(text[pos]))
      {
        ++pos;
      }
      if(pos >= n)
      {
        break;
      }
      if(text[pos] == '>')
      {
        ++pos;
        closed = true;
        break;
      }
      if(text[pos] == '/')
      {
        self_closing = true;
        ++pos;
        continue;
      }
      const size_t attr_start = pos;
      while(pos < n && !isSpace(text[pos]) && text[pos] != '=' && text[pos] != '>' &&
            text[pos] != '/')
      {
        ++pos;
      }
      const std::string attr_name = text.substr(attr_start, pos - attr_start);
      while(pos < n && isSpace(text[pos]))
      {
        ++pos;
      }
      if(pos >= n || text[pos] != '=')
      {
        // Attribute without a value: not well-formed. Skip the character.
        if(pos < n && text[pos] != '>' && text[pos] != '/')
        {
          ++pos;
        }
        continue;
      }
      ++pos;
      while(pos < n && isSpace(text[pos]))
      {
        ++pos;
      }
      if(pos >= n || (text[pos] != '"' && text[pos] != '\''))
      {
        return info;
      }
      const char quote = text[pos++];
      const auto value_end = text.find(quote, pos);
      if(value_end == std::string::npos)
      {
        return info;
      }
      const std::string attr_value = decodeXmlEntities(text.substr(pos, value_end - pos));
      pos = value_end + 1;

      if(depth == 0 && attr_name == "main_tree_to_execute")
      {
        info.main_tree_to_execute = attr_value;
      }
      if(depth == 1 && name == "BehaviorTree" && attr_name == "ID" &&
         info.first_tree_id.empty())
      {
        info.first_tree_id = attr_value;
      }
    }
    if(!closed)
    {
      break;
    }
    if(!self_closing)
    {
      ++depth;
    }
  }
  return info;
}

std::string readFile(const std::filesystem::path& path)
{
  std::ifstream file(path, std::ios::binary);
  if(!file)
  {
    throw std::runtime_error("cannot open file: " + path.string());
  }
  return std::string(std::istreambuf_iterator<char>(file), std::istreambuf_iterator<char>());
}

std::string chooseTreeId(const Options& options, const std::filesystem::path& xml_path,
                         const BT::BehaviorTreeFactory& factory)
{
  if(!options.tree.empty())
  {
    return options.tree;
  }
  const TopLevelInfo info = scanTopLevel(readFile(xml_path));
  if(!info.main_tree_to_execute.empty())
  {
    return info.main_tree_to_execute;
  }
  if(!info.first_tree_id.empty())
  {
    return info.first_tree_id;
  }
  // The main file has no <BehaviorTree> of its own (only <include>).
  const auto trees = factory.registeredBehaviorTrees();
  if(trees.empty())
  {
    throw std::runtime_error("the XML file contains no <BehaviorTree>");
  }
  return trees.front();
}

// ---------------------------------------------------------------------------
// Breakpoint release on stop
// ---------------------------------------------------------------------------

#ifdef FASTTICK_HAVE_ZMQ
/// Sends DISABLE_ALL_HOOKS to the publisher of this process.
///
/// A tick that waits at a breakpoint blocks the main thread. The publisher
/// wakes it up when a hook becomes disabled (enableAllHooks of the
/// publisher). The request header layout comes from
/// BT::Monitor::SerializeHeader in the BT.CPP protocol header.
bool releaseBreakpoints(unsigned port)
{
  void* context = zmq_ctx_new();
  if(context == nullptr)
  {
    return false;
  }
  bool replied = false;
  void* socket = zmq_socket(context, ZMQ_REQ);
  if(socket != nullptr)
  {
    const int linger = 0;
    const int timeout_ms = 500;
    zmq_setsockopt(socket, ZMQ_LINGER, &linger, sizeof(linger));
    zmq_setsockopt(socket, ZMQ_RCVTIMEO, &timeout_ms, sizeof(timeout_ms));
    zmq_setsockopt(socket, ZMQ_SNDTIMEO, &timeout_ms, sizeof(timeout_ms));
    const std::string endpoint = "tcp://127.0.0.1:" + std::to_string(port);
    if(zmq_connect(socket, endpoint.c_str()) == 0)
    {
      const std::string header = BT::Monitor::SerializeHeader(
          BT::Monitor::RequestHeader(BT::Monitor::RequestType::DISABLE_ALL_HOOKS));
      if(zmq_send(socket, header.data(), header.size(), 0) >= 0)
      {
        char buffer[64];
        int more = 0;
        do
        {
          if(zmq_recv(socket, buffer, sizeof(buffer), 0) < 0)
          {
            break;
          }
          replied = true;
          size_t more_size = sizeof(more);
          if(zmq_getsockopt(socket, ZMQ_RCVMORE, &more, &more_size) != 0)
          {
            break;
          }
        } while(more != 0);
      }
    }
    zmq_close(socket);
  }
  zmq_ctx_term(context);
  return replied;
}
#endif

/// Watches for a stop request while the main thread ticks. If the main
/// thread does not leave the tick loop soon after the signal, the watcher
/// releases blocked breakpoints of the publisher.
///
/// With a parent PID that is not 0, the watcher also requests a stop when
/// the parent process ends (the process then gets a new parent). This
/// stops an executor that a killed test process leaves behind.
class StopWatcher
{
public:
  StopWatcher(unsigned port, pid_t parent_pid) : port_(port), parent_pid_(parent_pid)
  {
    thread_ = std::thread([this] { run(); });
  }

  StopWatcher(const StopWatcher&) = delete;
  StopWatcher& operator=(const StopWatcher&) = delete;

  ~StopWatcher()
  {
    finish();
  }

  /// Tells the watcher that the tick loop ended, and joins its thread.
  void finish()
  {
    loop_finished_.store(true);
    if(thread_.joinable())
    {
      thread_.join();
    }
  }

private:
  void run()
  {
    using namespace std::chrono;
    std::optional<steady_clock::time_point> next_release;
    while(!loop_finished_.load())
    {
      if(parent_pid_ != 0 && getppid() != parent_pid_)
      {
        g_parent_exited.store(true);
      }
      if(stopRequested() && port_ != 0)
      {
        const auto now = steady_clock::now();
        if(!next_release)
        {
          // Give the tick loop a short time to see the request first.
          next_release = now + milliseconds(200);
        }
        else if(now >= *next_release)
        {
#ifdef FASTTICK_HAVE_ZMQ
          releaseBreakpoints(port_);
#endif
          next_release = now + milliseconds(500);
        }
      }
      std::this_thread::sleep_for(milliseconds(10));
    }
  }

  unsigned port_;
  pid_t parent_pid_;
  std::atomic<bool> loop_finished_{ false };
  std::thread thread_;
};

/// Halts the nodes that are not IDLE, from the root down.
///
/// BT::Tree::haltTree() is not used: it calls haltNode() on every node, and
/// haltNode() always runs the _onHalted script of the node
/// (TreeNode::haltNode in src/tree_node.cpp, Tree::haltTree in
/// src/bt_factory.cpp). haltTree() would thus run the _onHalted scripts of
/// nodes that never ran. A real application that ticks the tree again after
/// it completes does not do that.
void haltActiveNodes(BT::Tree& tree)
{
  BT::TreeNode* root = tree.rootNode();
  if(root == nullptr)
  {
    return;
  }
  // Pre-order: a halted parent halts and resets its children
  // (ControlNode::resetChildren, DecoratorNode::resetChild), so the
  // children are IDLE when the visitor gets to them.
  BT::applyRecursiveVisitor(root, [](BT::TreeNode* node) {
    if(node->status() != BT::NodeStatus::IDLE)
    {
      node->haltNode();
    }
  });
}

// ---------------------------------------------------------------------------
// Main program
// ---------------------------------------------------------------------------

/// Sleeps for the given time, in short steps, so that a stop request or
/// the deadline ends the sleep early.
void sleepInterruptible(std::chrono::milliseconds total,
                        std::optional<std::chrono::steady_clock::time_point> deadline)
{
  using namespace std::chrono;
  const auto end = steady_clock::now() + total;
  while(!stopRequested())
  {
    auto now = steady_clock::now();
    if(now >= end || (deadline && now >= *deadline))
    {
      return;
    }
    auto step = std::min<steady_clock::duration>(end - now, milliseconds(5));
    std::this_thread::sleep_for(step);
  }
}

int run(const Options& options, pid_t parent_pid)
{
  namespace fs = std::filesystem;
  using namespace std::chrono;

  if(!fs::is_regular_file(fs::path(options.xml)))
  {
    throw std::runtime_error("XML file not found: " + options.xml);
  }
  // XMLParser::loadFromFile (src/xml_parsing.cpp) calls
  // std::filesystem::absolute(filepath.parent_path()). For a bare file name
  // the parent path is empty, and absolute("") throws. An absolute path
  // avoids this.
  const fs::path xml_path = fs::absolute(fs::path(options.xml));

  BT::BehaviorTreeFactory factory;
  registerFastTickTestNodes(factory);
  // Relative <include path="..."> elements resolve from the directory of
  // the file (XMLParser::PImpl::loadDocImpl in src/xml_parsing.cpp).
  factory.registerBehaviorTreeFromFile(xml_path);

  const std::string tree_id = chooseTreeId(options, xml_path, factory);
  BT::Tree tree = factory.createTree(tree_id);

  // Construction order: publisher, then loggers. Destruction is in the
  // reverse order (see the end of this function).
  std::unique_ptr<BT::Groot2Publisher> publisher;
  if(options.port != 0)
  {
    try
    {
      publisher =
          std::make_unique<BT::Groot2Publisher>(tree, static_cast<unsigned>(options.port));
    }
    catch(const std::exception& error)
    {
      throw std::runtime_error("cannot start the publisher on ports " +
                               std::to_string(options.port) + " and " +
                               std::to_string(options.port + 1) + ": " + error.what());
    }
    if(options.heartbeat_ms)
    {
      // The heartbeat thread of the publisher already runs here. BT.CPP has
      // no constructor argument for the delay, so this is the only way.
      publisher->setMaxHeartbeatDelay(milliseconds(*options.heartbeat_ms));
    }
  }
  std::unique_ptr<BT::FileLogger2> file_logger;
  if(!options.btlog.empty())
  {
    file_logger = std::make_unique<BT::FileLogger2>(tree, fs::path(options.btlog));
  }
#ifdef FASTTICK_HAVE_SQLITE_LOGGER
  std::unique_ptr<BT::SqliteLogger> sqlite_logger;
  if(!options.sqlite.empty())
  {
    sqlite_logger = std::make_unique<BT::SqliteLogger>(tree, fs::path(options.sqlite));
  }
#else
  if(!options.sqlite.empty())
  {
    throw std::runtime_error("this BT.CPP build has no SqliteLogger");
  }
#endif

  std::cout << "READY port=" << options.port << std::endl;

  StopWatcher watcher(static_cast<unsigned>(options.port),
                      options.exit_with_parent ? parent_pid : 0);

  const auto start = steady_clock::now();
  std::optional<steady_clock::time_point> deadline;
  if(options.duration_ms > 0)
  {
    deadline = start + milliseconds(options.duration_ms);
  }

  uint64_t loops_done = 0;
  while(!stopRequested())
  {
    if(deadline && steady_clock::now() >= *deadline)
    {
      break;
    }
    const BT::NodeStatus status = tree.tickExactlyOnce();
    if(status != BT::NodeStatus::RUNNING)
    {
      ++loops_done;
      std::cout << "RESULT loop=" << loops_done << " status=" << BT::toStr(status)
                << std::endl;
      if(options.loops != 0 && loops_done >= options.loops)
      {
        break;
      }
      // Tree::tickRoot (src/bt_factory.cpp) already resets the root when
      // it completes, and the control nodes reset their children. This
      // call only resets a node that a custom implementation left active.
      haltActiveNodes(tree);
    }
    sleepInterruptible(milliseconds(options.tick_ms), deadline);
  }

  // A signal that comes after this point does not change the result.
  const int stop_signal = g_stop_signal.load();
  const bool parent_exited = g_parent_exited.load();
  watcher.finish();
  // Stop: halt the running nodes (the loggers record the transitions to
  // IDLE), then destroy the loggers, then the publisher.
  haltActiveNodes(tree);
  bool have_logger = file_logger != nullptr;
#ifdef FASTTICK_HAVE_SQLITE_LOGGER
  have_logger = have_logger || sqlite_logger != nullptr;
#endif
  if(have_logger)
  {
    // The writer threads of FileLogger2 and SqliteLogger end when the
    // destructor clears their loop flag, without a last pass over the
    // queue (FileLogger2::writerLoop in src/loggers/bt_file_logger_v2.cpp,
    // SqliteLogger::writerLoop in src/loggers/bt_sqlite_logger.cpp). The
    // last transitions can then be lost. A short pause lets the writers
    // empty their queues first (FileLogger2 wakes up at least every 10 ms).
    std::this_thread::sleep_for(milliseconds(100));
  }
#ifdef FASTTICK_HAVE_SQLITE_LOGGER
  sqlite_logger.reset();
#endif
  file_logger.reset();
  publisher.reset();

  if(stop_signal != 0)
  {
    std::cout << "STOPPED signal=" << stop_signal << std::endl;
  }
  else if(parent_exited)
  {
    std::cout << "STOPPED parent=exited" << std::endl;
  }
  else
  {
    std::cout << "DONE" << std::endl;
  }
  return kExitOk;
}

}  // namespace

int main(int argc, char** argv)
{
  // Read the parent PID first, before it can change.
  const pid_t parent_pid = getppid();
  installSignalHandlers();

  Options options;
  try
  {
    options = parseOptions(argc, argv);
  }
  catch(const UsageError& error)
  {
    std::cerr << "fasttick_executor: " << error.what() << "\n"
              << "Try 'fasttick_executor --help'." << std::endl;
    return kExitUsage;
  }
  if(options.help)
  {
    printUsage(std::cout);
    return kExitOk;
  }

  try
  {
    return run(options, parent_pid);
  }
  catch(const std::exception& error)
  {
    std::cerr << "fasttick_executor: error: " << error.what() << std::endl;
    return kExitRuntimeError;
  }
}
