"""Documentation checks: every installer and SSH-catalog error code is documented, and relative
Markdown links resolve. Run: python3 -I -m unittest discover -s tests/docs -v"""
import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
INSTALLER = (ROOT / "scripts/install-agent.sh").read_text()
SSH = (ROOT / "crates/hope-server/src/ssh.rs").read_text()
OPERATIONS = (ROOT / "docs/operations.md").read_text()


def installer_codes():
    return set(re.findall(r"\b(?:fail|err) ([A-Z][A-Z_]{3,})\b", INSTALLER))


def catalog_codes():
    return set(re.findall(r'\binfo\(\s*"([A-Z][A-Z_]+)"', SSH))


def outcome_codes():
    """Codes the server assigns directly via Outcome::code(...)."""
    return set(re.findall(r'Outcome::code\(\s*"([A-Z][A-Z_]+)"', SSH))


class OperationsCodes(unittest.TestCase):
    def test_sources_were_parsed(self):
        # Guards against a regex silently matching nothing after a refactor.
        self.assertGreaterEqual(len(installer_codes()), 25)
        self.assertGreaterEqual(len(catalog_codes()), 40)

    def test_every_installer_code_is_catalogued(self):
        self.assertEqual(installer_codes() - catalog_codes(), set())

    def test_every_server_outcome_code_is_catalogued(self):
        self.assertEqual(outcome_codes() - catalog_codes(), set())

    def test_every_code_has_a_table_row_in_operations(self):
        rows = set(re.findall(r"^\| `([A-Z][A-Z_]+)` \|", OPERATIONS, re.M))
        missing = (installer_codes() | catalog_codes()) - rows
        self.assertEqual(missing, set(), "codes missing from the docs/operations.md error table")

    def test_table_has_no_stale_codes(self):
        rows = set(re.findall(r"^\| `([A-Z][A-Z_]+)` \| (?:installer|server) \|", OPERATIONS, re.M))
        self.assertEqual(rows - (installer_codes() | catalog_codes()), set())


VAR = re.compile(r"\bHOPE_[A-Z][A-Z_]*[A-Z]\b")
# Environment names that are internal or test-only and deliberately not documented for operators.
UNDOCUMENTED = {
    "HOPE_INSTALL_ROOT", "HOPE_POLL_INTERVAL",   # installer test knobs
    "HOPE_PUBLIC_KEY", "HOPE_RELEASE_ID", "HOPE_INSTALL_PID",  # installer placeholders / internal
    "HOPE_ACTIVATION_ID",                        # set by the generated service definition
    "HOPE_COMMIT", "HOPE_MACOS_MINIMUM",         # build script internals
}


def production_sources():
    files = [p for p in (ROOT / "scripts").iterdir() if p.suffix in (".sh", ".py")]
    files += [ROOT / n for n in ("Dockerfile", "compose.yaml", "compose.desktop.yaml", ".env.example")]
    for crate in (ROOT / "crates").glob("*/src"):
        files += crate.rglob("*.rs")
    for path in files:
        text = path.read_text()
        if path.suffix == ".rs":
            text = text.split("#[cfg(test)]")[0]
        yield text


class EnvironmentVariables(unittest.TestCase):
    def test_every_production_variable_is_documented(self):
        used = set().union(*(set(VAR.findall(t)) for t in production_sources()))
        used = {v for v in used if not v.startswith("HOPE_TEST_")}
        self.assertGreater(len(used), 15)
        self.assertEqual(sorted(v for v in used - UNDOCUMENTED if v not in OPERATIONS), [])

    def test_documented_variables_exist_in_code(self):
        everything = "\n".join(p.read_text() for p in ROOT.glob("**/*") if p.is_file() and p.suffix in (".rs", ".sh", ".py", ".yaml", ".example") and not any(x in p.parts for x in ("target", "node_modules", ".git")))
        named = set()
        for doc in ("README.md", "docs/architecture.md", "docs/operations.md"):
            named |= set(VAR.findall((ROOT / doc).read_text()))
        self.assertEqual(sorted(v for v in named if v not in everything), [])


def slugs_of(path):
    text = re.sub(r"```.*?```", "", path.read_text(), flags=re.S)
    return {re.sub(r"[^a-z0-9 _-]", "", h.lower()).replace(" ", "-") for h in re.findall(r"^#{1,6} (.+)$", text, re.M)}


class MarkdownLinks(unittest.TestCase):
    FILES = ["README.md", "docs/architecture.md", "docs/operations.md", "docs/validation.md"]

    def test_relative_links_and_anchors_resolve(self):
        problems = []
        for name in self.FILES:
            path = ROOT / name
            body = re.sub(r"```.*?```", "", path.read_text(), flags=re.S)
            for target in re.findall(r"\]\(([^)\s]+)\)", body):
                if re.match(r"[a-z]+:", target):
                    continue
                file_part, _, fragment = target.partition("#")
                resolved = (path.parent / file_part).resolve() if file_part else path
                if not resolved.exists():
                    problems.append("%s -> %s (missing file)" % (name, target))
                elif fragment and resolved.suffix == ".md" and fragment not in slugs_of(resolved):
                    problems.append("%s -> %s (missing anchor)" % (name, target))
        self.assertEqual(problems, [])


if __name__ == "__main__":
    unittest.main()
