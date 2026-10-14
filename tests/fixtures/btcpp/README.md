# BehaviorTree.CPP XML fixtures

The files in this directory come from BehaviorTree.CPP 4.9.0
(<https://github.com/BehaviorTree/BehaviorTree.CPP>, tag `4.9.0`, commit `3ff6a32`).
BehaviorTree.CPP is MIT-licensed. Its license text is below.

The script `tools/extract_btcpp_xml.py` made these files. Do not edit them by hand;
run the script again instead:

```sh
python3 tools/extract_btcpp_xml.py --btcpp /path/to/BehaviorTree.CPP
```

## Contents

- `examples/`, `tests/`, `fuzzing/corpus/bt_corpus/`: copies of the `*.xml` files at the
  same paths in BehaviorTree.CPP.
- `package.xml`: a copy of the ROS package manifest of BehaviorTree.CPP.
- `inline/<source file stem>_<n>.xml`: the XML text of each C++ raw string literal
  (`R"(...)"`) that contains `<root`, from the C++ files in `examples/`, `tests/` and
  `sample_nodes/`. `<n>` counts these literals in each source file, from 1. The file
  names keep the names of the BehaviorTree.CPP source files.

The script copies the bytes exactly. Some fixtures are not well-formed XML on purpose;
the list and the reasons are in `MALFORMED_FIXTURES` in
`crates/fasttick-core/tests/xml_roundtrip.rs`.

## License of BehaviorTree.CPP

```text
The MIT License (MIT)

Copyright (c) 2015 - 2018 Michele Colledanchise

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
