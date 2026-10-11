#!/usr/bin/env python3
"""Checks that workflows pass the environment variables their scripts require.

This exists because of a real failure: `pages.yml` passed `ARM_URL` while
`write_feed.py` read `ARM64_URL`, so the first release published successfully but
never wrote its update feed. Every installed copy then got a 404 from the feed and
reported a curl error, which reads as a network problem rather than a typo in one
workflow file.

The mismatch is invisible to YAML parsing, to actionlint, and to `shellcheck`. Only
comparing the two sides catches it.
"""

from __future__ import annotations

import pathlib
import re
import sys

try:
    import yaml
except ImportError:
    print("  PyYAML not installed; skipping (pip install pyyaml)", file=sys.stderr)
    sys.exit(0)

root = pathlib.Path(__file__).resolve().parents[2]
workflows = sorted((root / ".github" / "workflows").glob("*.yml"))

# How a script declares what it needs. Both forms are in use: `required("X")` reads
# and fails loudly, and `os.environ.get("X")` is optional.
READS_REQUIRED = re.compile(r'required\(\s*"([A-Z][A-Z0-9_]*)"\s*\)')
READS_OPTIONAL = re.compile(r'os\.environ(?:\.get)?[(\[]\s*"([A-Z][A-Z0-9_]*)"')
# The shell form: `${SOMETHING:?message}` fails at runtime if unset.
SHELL_REQUIRED = re.compile(r"\$\{([A-Z][A-Z0-9_]*):\?")

problems: list[str] = []
checked = 0


def script_names_in(text: str) -> set[str]:
    """Finds the checked-in scripts a workflow's run steps invoke."""
    found = set()
    for match in re.findall(r"(Scripts/checks/[a-z_]+\.py|Scripts/[a-z-]+\.sh)", text):
        found.add(match)
    return found


for workflow in workflows:
    try:
        document = yaml.safe_load(workflow.read_text(encoding="utf-8"))
    except yaml.YAMLError as error:
        problems.append(f"{workflow.name}: not valid YAML ({error})")
        continue

    for job_name, job in (document.get("jobs") or {}).items():
        if not isinstance(job, dict):
            continue
        for step in job.get("steps") or []:
            if not isinstance(step, dict):
                continue

            # The step's own environment, plus the workflow-level one it inherits.
            env = set()
            for source in (document.get("env"), job.get("env"), step.get("env")):
                if isinstance(source, dict):
                    env |= set(source)

            script = step.get("run") or ""
            needed: set[str] = set()

            for path in script_names_in(script):
                target = root / path
                if not target.exists():
                    problems.append(f"{workflow.name}: step runs a missing script {path}")
                    continue
                text = target.read_text(encoding="utf-8")
                needed |= set(READS_REQUIRED.findall(text))
                # Optional reads are checked too: a name that is never passed is
                # usually a rename that was not finished.
                needed |= set(READS_OPTIONAL.findall(text)) - {"PATH", "HOME", "TMPDIR", "PWD"}

            # Inline shell that requires a variable with `:?`.
            needed |= set(SHELL_REQUIRED.findall(script))
            if not needed:
                continue

            checked += 1
            missing = sorted(needed - env)
            if missing:
                problems.append(
                    f"{workflow.name} / {step.get('name', 'unnamed step')}: "
                    f"the script needs {', '.join(missing)} but the step does not pass "
                    f"{'them' if len(missing) > 1 else 'it'}"
                )

print(f"  {checked} step(s) that pass environment variables to a script")

if problems:
    for problem in problems:
        print(f"  {problem}", file=sys.stderr)
    sys.exit(1)

print("  every workflow passes the variables its scripts read")
