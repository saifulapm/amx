"""Name the tests that passed only on a retry, from nextest's JUnit report.

CI appends what this prints to the job summary, under a `flaky` heading that
is there whether any test was flaky or not:

    python3 .github/scripts/flaky.py target/nextest/ci/junit.xml
"""

import sys
import xml.etree.ElementTree as ET

print("## flaky\n")
try:
    suites = ET.parse(sys.argv[1]).getroot()
except (OSError, ET.ParseError) as why:
    print(f"No test report to read: {why}")
    sys.exit(0)

flaky = [
    f"{case.get('classname')} {case.get('name')}"
    for case in suites.iter("testcase")
    if case.find("flakyFailure") is not None or case.find("flakyError") is not None
]
if flaky:
    for name in flaky:
        print(f"- `{name}`")
else:
    print("None: every test passed on its first try.")
